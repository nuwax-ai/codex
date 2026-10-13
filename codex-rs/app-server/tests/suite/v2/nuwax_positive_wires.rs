//! Fork (nuwax-codex): Chat and Responses positive controls for the `NUWAX_*`
//! environment group over a REAL child-process app-server, complementing
//! `nuwax_positive.rs` (Anthropic):
//!
//! * a tool round trip on the Chat wire — the model's `tool_calls` chunk
//!   executes the built-in `exec_command` tool and the tool output returns to
//!   the provider in a second credentialed POST;
//! * a tool round trip on the Responses wire through the REAL Responses
//!   adapter (`input` item array with `function_call_output` history, never a
//!   Chat `messages` payload), with rig-decoded usage reaching the shared v2
//!   accounting;
//! * an in-flight stream interrupt followed by two resumes (warm in the same
//!   child, then cold in a fresh child), with the interrupted attempt the
//!   only extra wire request.
//!
//! Every wire assertion (path, bearer credential, model, output cap, tool
//! advertisement, tool-result history) runs against the recorded HTTP
//! requests, not against the mocked responses. The child disables
//! remote-control startup via the daemon's internal env marker for the same
//! reason as `nuwax_positive.rs`.

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadTokenUsageUpdatedNotification;
use codex_app_server_protocol::TokenUsageBreakdown;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_app_server_transport::REMOTE_CONTROL_DISABLED_ENV_VAR;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const READ_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 60);
const TEST_MODEL: &str = "nuwax-wire-model";
const TEST_KEY: &str = "nuwax-wire-key";
const TOOL_MARKER: &str = "nuwax-tool-ok";
const CHAT_ANSWER: &str = "chat wire answered";
const RESPONSES_ANSWER: &str = "responses wire answered";
const TEST_KEY_HEADER: &str = "Bearer nuwax-wire-key";

/// Captures every matched request; the wire assertions run against this log,
/// not against the mocked responses.
#[derive(Clone, Default)]
struct RequestLog(Arc<Mutex<Vec<wiremock::Request>>>);

impl wiremock::Match for RequestLog {
    fn matches(&self, request: &wiremock::Request) -> bool {
        self.0
            .lock()
            .expect("request log lock")
            .push(request.clone());
        true
    }
}

impl RequestLog {
    fn all(&self) -> Vec<wiremock::Request> {
        self.0.lock().expect("request log lock").clone()
    }
}

fn chat_frame(choice: Value) -> Value {
    json!({
        "id": "chatcmpl-nuwax", "object": "chat.completion.chunk", "created": 1,
        "model": TEST_MODEL, "choices": [choice],
    })
}

fn chat_usage_frame() -> Value {
    json!({
        "id": "chatcmpl-nuwax", "object": "chat.completion.chunk", "created": 1,
        "model": TEST_MODEL, "choices": [],
        "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6},
    })
}

fn chat_sse(frames: Vec<Value>) -> String {
    let mut payload = frames
        .into_iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>();
    payload.push_str("data: [DONE]\n\n");
    payload
}

fn exec_command_arguments() -> String {
    serde_json::to_string(&json!({"cmd": format!("echo {TOOL_MARKER}")}))
        .expect("exec_command arguments")
}

/// The model asks to run one harmless shell command; the terminal
/// `finish_reason: "tool_calls"` leaves the turn open for the tool result.
fn chat_tool_call_sse() -> String {
    chat_sse(vec![
        chat_frame(
            json!({"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}),
        ),
        chat_frame(json!({"index": 0, "delta": {"tool_calls": [
            {"index": 0, "id": "call-nuwax-1", "type": "function",
             "function": {"name": "exec_command", "arguments": exec_command_arguments()}}
        ]}, "finish_reason": null})),
        chat_frame(json!({"index": 0, "delta": {}, "finish_reason": "tool_calls"})),
        chat_usage_frame(),
    ])
}

fn chat_final_sse(answer: &str) -> String {
    chat_sse(vec![
        chat_frame(
            json!({"index": 0, "delta": {"role": "assistant", "content": answer}, "finish_reason": null}),
        ),
        chat_frame(json!({"index": 0, "delta": {}, "finish_reason": "stop"})),
        chat_usage_frame(),
    ])
}

fn responses_completed_with_usage(id: &str) -> Value {
    json!({
        "type": "response.completed",
        "response": {"id": id, "usage": {"input_tokens": 4, "output_tokens": 2, "total_tokens": 6}}
    })
}

/// Same tool round trip as the Chat fixture, in real Responses event shape.
fn responses_tool_call_sse() -> String {
    responses::sse(vec![
        responses::ev_response_created("resp-nuwax-1"),
        responses::ev_function_call("call-nuwax-1", "exec_command", &exec_command_arguments()),
        responses_completed_with_usage("resp-nuwax-1"),
    ])
}

fn responses_final_sse() -> String {
    responses::sse(vec![
        responses::ev_response_created("resp-nuwax-2"),
        responses::ev_assistant_message("msg-nuwax-1", RESPONSES_ANSWER),
        responses_completed_with_usage("resp-nuwax-2"),
    ])
}

/// Environment changes apply only to this explicitly controlled child. The
/// output cap seeds `NUWAX_MAX_OUTPUT_TOKENS` so both wires carry a real
/// provider-level cap field in their request bodies.
async fn child(
    codex_home: &TempDir,
    server_uri: &str,
    wire_api: &str,
    extra: &[(&str, &str)],
) -> Result<TestAppServer> {
    let base_url = format!("{server_uri}/v1");
    let mut overrides: Vec<(&str, Option<&str>)> = [
        "NUWAX_BASE_URL",
        "NUWAX_WIRE_API",
        "NUWAX_API_KEY",
        "NUWAX_MODEL",
        "NUWAX_MAX_OUTPUT_TOKENS",
        "NUWAX_REQUEST_MAX_RETRIES",
        "NUWAX_STREAM_MAX_RETRIES",
        "NUWAX_STREAM_IDLE_TIMEOUT_MS",
    ]
    .iter()
    .map(|name| (*name, None))
    .collect();
    for (name, value) in extra {
        overrides.retain(|(existing, _)| existing != name);
        overrides.push((name, Some(value)));
    }
    overrides.extend([
        ("NUWAX_BASE_URL", Some(base_url.as_str())),
        ("NUWAX_WIRE_API", Some(wire_api)),
        ("NUWAX_API_KEY", Some(TEST_KEY)),
        ("NUWAX_MODEL", Some(TEST_MODEL)),
        ("NUWAX_MAX_OUTPUT_TOKENS", Some("2048")),
        (REMOTE_CONTROL_DISABLED_ENV_VAR, Some("1")),
    ]);
    TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_managed_config()
        .with_env_overrides(&overrides)
        .build_initialized_with_timeout(READ_TIMEOUT)
        .await
}

fn assert_common_wire_shape(request: &wiremock::Request, expected_path: &str) -> Result<Value> {
    let body: Value = serde_json::from_slice(&request.body).context("request body json")?;
    assert_eq!(
        (
            request.method.as_str(),
            request.url.path(),
            body["model"].as_str(),
            request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
        ),
        (
            "POST",
            expected_path,
            Some(TEST_MODEL),
            Some(TEST_KEY_HEADER)
        ),
        "every wire request carries the env credential, model and endpoint"
    );
    Ok(body)
}

async fn completed_turn(
    app: &mut TestAppServer,
    thread_id: &str,
    prompt: &str,
) -> Result<TurnCompletedNotification> {
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
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    assert_eq!(completed.turn.error, None);
    Ok(completed)
}

#[tokio::test]
async fn chat_env_group_executes_a_tool_round_trip_over_the_real_chat_wire() -> Result<()> {
    let server = MockServer::start().await;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(chat_tool_call_sse(), "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(chat_final_sse(CHAT_ANSWER), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let codex_home = TempDir::new()?;
    let mut app = child(&codex_home, &server.uri(), "chat", &[]).await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    let completed = completed_turn(&mut app, &thread.id, "run the marker command").await?;
    let answers: Vec<_> = completed
        .turn
        .items
        .iter()
        .filter_map(|item| match item {
            codex_app_server_protocol::ThreadItem::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(answers, vec![CHAT_ANSWER]);
    app.shutdown_gracefully().await?;

    let requests = log.all();
    assert_eq!(requests.len(), 2, "tool call + tool result round trip");
    let first = assert_common_wire_shape(&requests[0], "/v1/chat/completions")?;
    assert_eq!(
        first["max_tokens"], 2048,
        "NUWAX_MAX_OUTPUT_TOKENS seeds the chat cap"
    );
    let advertised: Vec<&str> = first["tools"]
        .as_array()
        .context("chat request advertises tools")?
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(
        advertised.contains(&"exec_command"),
        "the executed tool must be advertised to the model: {advertised:?}"
    );
    let second = assert_common_wire_shape(&requests[1], "/v1/chat/completions")?;
    assert_eq!(second["max_tokens"], 2048);
    let tool_results: Vec<&str> = second["messages"]
        .as_array()
        .context("chat history messages")?
        .iter()
        .filter(|message| message["role"] == "tool" && message["tool_call_id"] == "call-nuwax-1")
        .filter_map(|message| message["content"].as_str())
        .collect();
    assert!(
        tool_results
            .iter()
            .any(|content| content.contains(TOOL_MARKER)),
        "the executed command's output returns to the provider: {tool_results:?}"
    );
    Ok(())
}

#[tokio::test]
async fn responses_env_group_executes_a_tool_round_trip_over_the_real_responses_adapter()
-> Result<()> {
    let server = MockServer::start().await;
    let log = RequestLog::default();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(responses_tool_call_sse(), "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(log.clone())
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(responses_final_sse(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let codex_home = TempDir::new()?;
    let mut app = child(&codex_home, &server.uri(), "responses", &[]).await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;

    // Raw request + notification loop: the rig-decoded usage from BOTH
    // responses must reach the shared v2 accounting before completion.
    let environment = app.auto_env_params()?;
    let _: TurnStartResponse = app
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                client_user_message_id: None,
                input: vec![V2UserInput::Text {
                    text: "run the marker command".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Some(vec![environment]),
                ..Default::default()
            },
        })
        .await?;
    let mut token_usage_notifications = Vec::new();
    let mut completed = None;
    timeout(READ_TIMEOUT, async {
        loop {
            let notification = app
                .read_stream_until_matching_notification(
                    "token usage or turn completion",
                    |notification| {
                        matches!(
                            notification.method.as_str(),
                            "thread/tokenUsage/updated" | "turn/completed"
                        )
                    },
                )
                .await?;
            match notification.method.as_str() {
                "thread/tokenUsage/updated" => token_usage_notifications
                    .push(notification.params.clone().context("token usage params")?),
                _ => {
                    completed = Some(
                        serde_json::from_value::<TurnCompletedNotification>(
                            notification
                                .params
                                .clone()
                                .context("turn completed params")?,
                        )
                        .context("turn/completed shape")?,
                    );
                    return Ok::<(), anyhow::Error>(());
                }
            }
        }
    })
    .await??;
    let completed = completed.expect("turn/completed observed");
    assert_eq!(
        (completed.turn.status, completed.turn.error),
        (TurnStatus::Completed, None)
    );
    let answers: Vec<_> = completed
        .turn
        .items
        .iter()
        .filter_map(|item| match item {
            codex_app_server_protocol::ThreadItem::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(answers, vec![RESPONSES_ANSWER]);
    let usage: ThreadTokenUsageUpdatedNotification = serde_json::from_value(
        token_usage_notifications
            .last()
            .context("rig-decoded usage must reach thread/tokenUsage/updated")?
            .clone(),
    )
    .context("thread/tokenUsage/updated shape")?;
    assert_eq!(
        (usage.thread_id.as_str(), usage.turn_id.as_str()),
        (thread.id.as_str(), completed.turn.id.as_str())
    );
    assert_eq!(
        (usage.token_usage.last, usage.token_usage.total),
        (
            TokenUsageBreakdown {
                input_tokens: 4,
                output_tokens: 2,
                total_tokens: 6,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                reasoning_output_tokens: 0,
            },
            TokenUsageBreakdown {
                input_tokens: 8,
                output_tokens: 4,
                total_tokens: 12,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                reasoning_output_tokens: 0,
            },
        ),
        "both Responses completions must reach shared usage accounting"
    );
    app.shutdown_gracefully().await?;

    let requests = log.all();
    assert_eq!(requests.len(), 2, "tool call + tool result round trip");
    for request in &requests {
        let body = assert_common_wire_shape(request, "/v1/responses")?;
        assert_eq!(
            body["max_output_tokens"], 2048,
            "NUWAX_MAX_OUTPUT_TOKENS seeds the responses cap"
        );
        // The real Responses adapter sends an `input` item array and never a
        // Chat `messages` payload.
        assert!(
            body["input"].is_array(),
            "responses wire requests carry an input item array: {}",
            serde_json::to_string(&body["input"])?
        );
        assert!(body.get("messages").is_none());
    }
    let second = assert_common_wire_shape(&requests[1], "/v1/responses")?;
    let tool_result = second["input"]
        .as_array()
        .context("responses history items")?
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == "call-nuwax-1")
        .context("the executed call's function_call_output item")?;
    let output = tool_result["output"]
        .as_str()
        .context("function_call_output text")?;
    assert!(
        output.contains(TOOL_MARKER),
        "the executed call's output returns to the provider: {tool_result}"
    );
    Ok(())
}

#[tokio::test]
async fn env_group_stream_interrupt_cancels_the_request_and_two_resumes_continue_the_thread()
-> Result<()> {
    let server = MockServer::start().await;
    let log = RequestLog::default();
    // The first attempt's response never arrives within the test's lifetime;
    // only the interrupt can end the turn.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            chat_final_sse("interrupted then resumed"),
            "text/event-stream",
        ))
        .mount(&server)
        .await;

    let codex_home = TempDir::new()?;
    // Zero retries on both layers: the interrupted attempt must be the only
    // extra wire request, never a masked resend.
    let mut app = child(
        &codex_home,
        &server.uri(),
        "chat",
        &[
            ("NUWAX_REQUEST_MAX_RETRIES", "0"),
            ("NUWAX_STREAM_MAX_RETRIES", "0"),
        ],
    )
    .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    let environment = app.auto_env_params()?;
    let TurnStartResponse { turn } = app
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                client_user_message_id: None,
                input: vec![V2UserInput::Text {
                    text: "stall this turn".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Some(vec![environment]),
                ..Default::default()
            },
        })
        .await?;
    let _: codex_app_server_protocol::TurnStartedNotification =
        timeout(READ_TIMEOUT, app.read_notification("turn/started")).await??;
    // turn/started fires before the first POST leaves the process; only
    // interrupt once the stalled attempt has actually reached the provider,
    // so the zero-retry budget can be asserted deterministically.
    timeout(Duration::from_secs(10), async {
        loop {
            if log.all().len() == 1 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;
    let _: TurnInterruptResponse = app
        .request(|request_id| ClientRequest::TurnInterrupt {
            request_id,
            params: TurnInterruptParams {
                thread_id: thread.id.clone(),
                turn_id: turn.id.clone(),
            },
        })
        .await?;
    let interrupted: TurnCompletedNotification =
        timeout(READ_TIMEOUT, app.read_notification("turn/completed")).await??;
    assert_eq!(
        (interrupted.turn.status, interrupted.turn.error.is_none()),
        (TurnStatus::Interrupted, true)
    );
    assert_eq!(
        log.all().len(),
        1,
        "the interrupted attempt is the only request"
    );

    // Warm resume in the same child.
    let resume_id = app
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let _: ThreadResumeResponse = timeout(READ_TIMEOUT, app.read_response(resume_id)).await??;
    completed_turn(&mut app, &thread.id, "warm resume continues").await?;
    let read_id = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: thread.id.clone(),
            include_turns: true,
        })
        .await?;
    let persisted: ThreadReadResponse = timeout(READ_TIMEOUT, app.read_response(read_id)).await??;
    let rollout_path = persisted.thread.path.clone().expect("durable rollout");
    assert_eq!(persisted.thread.turns.len(), 2);
    app.shutdown_gracefully().await?;

    // Cold resume in a fresh child with the same startup group.
    let mut resumed_child = child(
        &codex_home,
        &server.uri(),
        "chat",
        &[
            ("NUWAX_REQUEST_MAX_RETRIES", "0"),
            ("NUWAX_STREAM_MAX_RETRIES", "0"),
        ],
    )
    .await?;
    let resume_id = resumed_child
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let resumed: ThreadResumeResponse =
        timeout(READ_TIMEOUT, resumed_child.read_response(resume_id)).await??;
    assert_eq!(
        (resumed.model.as_str(), resumed.model_provider.as_str()),
        (TEST_MODEL, "nuwax_env")
    );
    completed_turn(&mut resumed_child, &thread.id, "cold resume continues").await?;
    resumed_child.shutdown_gracefully().await?;

    let requests = log.all();
    assert_eq!(requests.len(), 3, "stalled + warm resume + cold resume");
    let prompts = [
        "stall this turn",
        "warm resume continues",
        "cold resume continues",
    ];
    for (request, prompt) in requests.iter().zip(prompts) {
        let body = assert_common_wire_shape(request, "/v1/chat/completions")?;
        assert_eq!(body["max_tokens"], 2048);
        let user_texts: Vec<&str> = body["messages"]
            .as_array()
            .context("chat history messages")?
            .iter()
            .filter(|message| message["role"] == "user")
            .filter_map(|message| match &message["content"] {
                Value::String(text) => Some(text.as_str()),
                Value::Array(parts) => parts.iter().find_map(|part| part["text"].as_str()),
                _ => None,
            })
            .collect();
        assert!(
            user_texts.contains(&prompt),
            "this attempt must carry its initiating prompt: {user_texts:?}"
        );
    }
    let persisted = std::fs::read_to_string(rollout_path)?;
    assert!(
        !persisted.contains(TEST_KEY),
        "the referenced credential must never persist into the rollout"
    );
    let config = std::fs::read_to_string(codex_home.path().join("config.toml")).unwrap_or_default();
    assert!(
        !config.contains(TEST_KEY),
        "the credential must never persist into config"
    );
    Ok(())
}

/// Closes the typed-environment acceptance chain's last gap: compaction and
/// post-compaction recall must ride the SAME `NUWAX_*` Chat wire (credentialed
/// request per attempt, cap field, usage accounting), and the credential must
/// stay out of the persisted rollout and config.
#[tokio::test]
async fn env_group_compact_and_recall_keep_the_typed_wire_and_usage_accounting() -> Result<()> {
    let server = MockServer::start().await;
    let log = RequestLog::default();
    let passphrase = "nuwax-compact-passphrase";
    for (times, body) in [
        (1, chat_final_sse("OK")),
        (1, chat_final_sse("summary: the passphrase was agreed")),
        (1, chat_final_sse(passphrase)),
    ] {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(log.clone())
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .up_to_n_times(times)
            .mount(&server)
            .await;
    }

    let codex_home = TempDir::new()?;
    let mut app = child(&codex_home, &server.uri(), "chat", &[]).await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    completed_turn(
        &mut app,
        &thread.id,
        &format!("请记住暗号：{passphrase}。只回复OK。"),
    )
    .await?;

    let compact_id = app
        .send_thread_compact_start_request(ThreadCompactStartParams {
            thread_id: thread.id.clone(),
        })
        .await?;
    let _: ThreadCompactStartResponse = timeout(READ_TIMEOUT, app.read_response(compact_id)).await??;
    // The compact sampling is exchange 2 on the SAME typed wire; wait until
    // it has actually reached the provider before the recall turn.
    timeout(Duration::from_secs(10), async {
        loop {
            if log.all().len() >= 2 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;

    let recalled = completed_turn(&mut app, &thread.id, "暗号是什么？直接回答暗号本身。").await?;
    let answers: Vec<&str> = recalled
        .turn
        .items
        .iter()
        .filter_map(|item| match item {
            codex_app_server_protocol::ThreadItem::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        answers.iter().any(|text| text.contains(passphrase)),
        "the passphrase must survive compaction: {answers:?}"
    );
    app.shutdown_gracefully().await?;

    let requests = log.all();
    assert!(requests.len() >= 3, "learn + compact + recall exchanges");
    for (index, request) in requests.iter().enumerate() {
        let body = assert_common_wire_shape(request, "/v1/chat/completions")?;
        assert_eq!(body["max_tokens"], 2048, "exchange {index} keeps the env cap");
    }
    let mut leaked_rollouts = Vec::new();
    let mut pending = vec![codex_home.path().to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "jsonl")
                && std::fs::read_to_string(&path)
                    .unwrap_or_default()
                    .contains(TEST_KEY)
            {
                leaked_rollouts.push(path);
            }
        }
    }
    assert!(
        leaked_rollouts.is_empty(),
        "the credential must never persist into the rollouts: {leaked_rollouts:?}"
    );
    let config = std::fs::read_to_string(codex_home.path().join("config.toml")).unwrap_or_default();
    assert!(
        !config.contains(TEST_KEY),
        "the credential must never persist into config"
    );
    Ok(())
}
