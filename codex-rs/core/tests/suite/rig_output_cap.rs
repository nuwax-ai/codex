//! Core public-path coverage for output-cap terminals on all three bridge
//! protocols (Chat, Anthropic, Responses passthrough).
//!
//! D3 acceptance (rig-stability follow-up): the bridge wire tests prove the
//! terminal error mapping with direct stream calls; these tests drive the
//! FULL core sampling loop (test_codex) with non-zero request/stream retry
//! budgets configured, so "exactly one POST" proves core classifies the
//! budget terminal as non-retryable instead of paying for another sample.
//! Partial output stays visible, no truncated tool executes, and frames
//! arriving after the terminal cannot overwrite the error.
//!
//! Feature gate: run with
//! `just test -p codex-core --features rust-rig -E 'test(rig_output_cap)'`
//! and verify the selected count is non-zero.

#![cfg(feature = "rust-rig")]

use anyhow::Context;
use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageRecord;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use wiremock::MockServer;

/// Retry budgets are deliberately NON-zero: if core ever reclassified the
/// cap terminal as retryable, every test below would observe extra POSTs.
fn output_cap_provider(base_url: &str, wire_api: WireApi) -> ModelProviderInfo {
    let protocol_name = match wire_api {
        WireApi::Chat => "chat",
        WireApi::Responses => "responses",
        WireApi::Anthropic => "anthropic",
    };
    ModelProviderInfo {
        capabilities: None,
        name: format!("rig-cap-{protocol_name}"),
        base_url: Some(format!("{base_url}/v1")),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        experimental_bridge: None,
        provider_id: Some("output-cap-test".into()),
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(2),
        stream_max_retries: Some(2),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        include_internal_metadata: false,
        max_output_tokens: Some(64),
        hosted_results_replay: None,
    }
}

#[derive(Default)]
struct TurnOutcome {
    agent_deltas: String,
    final_agent_message: Option<String>,
    errors: Vec<String>,
    turn_error: Option<String>,
    exec_begun: usize,
    completed_agent_items: usize,
    completed_responses: usize,
    completed_tool_items: Vec<String>,
    cap_partials: Vec<codex_protocol::protocol::CapPartialEvent>,
    /// Kinds of cap/error/complete events in arrival order.
    terminal_order: Vec<&'static str>,
}

/// Runs one user turn to its TurnComplete, collecting everything needed to
/// assert the cap-terminal contract: partial deltas, the terminal error, and
/// absence of tool execution. Fails (rather than hangs) after 60s per event.
async fn run_turn_until_complete(test: &TestCodex, prompt: &str) -> Result<TurnOutcome> {
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: prompt.into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut outcome = TurnOutcome::default();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(60), test.codex.next_event())
            .await
            .context("timed out waiting for a turn event")?
            .context("event stream ended before TurnComplete")?;
        match event.msg {
            EventMsg::AgentMessageContentDelta(delta) => {
                outcome.agent_deltas.push_str(&delta.delta);
            }
            EventMsg::AgentMessage(message) => outcome.final_agent_message = Some(message.message),
            EventMsg::Error(error) => {
                outcome.errors.push(error.message);
                outcome.terminal_order.push("error");
            }
            EventMsg::CapPartial(event) => {
                outcome.cap_partials.push(event);
                outcome.terminal_order.push("cap_partial");
            }
            EventMsg::ExecCommandBegin(_) => outcome.exec_begun += 1,
            EventMsg::RawResponseCompleted(_) => outcome.completed_responses += 1,
            EventMsg::ItemCompleted(item) => match item.item {
                TurnItem::AgentMessage(_) => outcome.completed_agent_items += 1,
                TurnItem::DynamicToolCall(call) => outcome.completed_tool_items.push(call.tool),
                _ => {}
            },
            EventMsg::TurnComplete(complete) => {
                outcome.turn_error = complete.error.map(|error| error.message);
                outcome.terminal_order.push("turn_complete");
                return Ok(outcome);
            }
            _ => {}
        }
    }
}

/// Asserts the shared cap-terminal contract. `expected_posts` counts the
/// attempts the server observed; 1 proves no core resampling.
fn assert_cap_terminal(outcome: &TurnOutcome, expected_posts: usize, partial_text: &str) {
    let needle = "Output token limit reached";
    let terminal = outcome.turn_error.as_deref().unwrap_or_else(|| {
        panic!(
            "cap terminal must surface as a turn error, got success with message {:?}",
            outcome.final_agent_message
        )
    });
    assert!(
        terminal.contains(needle),
        "terminal error must name the output cap, got: {terminal}"
    );
    assert!(
        outcome
            .errors
            .iter()
            .all(|message| message.contains(needle)),
        "no other error class may appear: {:?}",
        outcome.errors
    );
    assert_eq!(
        outcome.errors.len(),
        1,
        "exactly one terminal error event: {:?}",
        outcome.errors
    );
    assert_eq!(
        outcome.final_agent_message, None,
        "partial output must not become an ordinary final AgentMessage"
    );
    assert_eq!(
        (outcome.completed_agent_items, outcome.completed_responses),
        (0, 0),
        "a cap must not complete pending agent items or the response"
    );

    assert_eq!(
        outcome.agent_deltas, partial_text,
        "partial output streamed before the cap stays visible"
    );
    assert_eq!(
        outcome.completed_tool_items,
        Vec::<String>::new(),
        "a truncated tool call must never complete"
    );
    assert_eq!(
        outcome.exec_begun, 0,
        "a truncated tool call must never execute"
    );
    assert_eq!(
        expected_posts, 1,
        "core must not resample a budget terminal despite the retry budget"
    );
}

/// Captures every matched request for deep assertions.
#[derive(Clone, Default)]
struct RequestLog(std::sync::Arc<std::sync::Mutex<Vec<wiremock::Request>>>);

impl RequestLog {
    fn requests(&self) -> Vec<wiremock::Request> {
        self.0.lock().expect("read captured requests").clone()
    }

    fn posts(&self) -> usize {
        self.0.lock().expect("count captured requests").len()
    }

    fn first_body(&self) -> Value {
        serde_json::from_slice(&self.requests()[0].body).expect("parse captured request body")
    }
}

impl wiremock::Match for RequestLog {
    fn matches(&self, request: &wiremock::Request) -> bool {
        self.0
            .lock()
            .expect("capture request")
            .push(request.clone());
        true
    }
}

async fn mount_capturing_response(
    server: &MockServer,
    path: &str,
    body: String,
) -> Result<RequestLog> {
    let log = RequestLog::default();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(path))
        .and(log.clone())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(body, "text/event-stream"),
        )
        .mount(server)
        .await;
    Ok(log)
}

async fn cap_test_instance(base_url: &str, wire_api: WireApi) -> Result<(TestCodex, MockServer)> {
    cap_test_instance_with(base_url, wire_api, /*cap_partial*/ false).await
}

async fn cap_test_instance_with(
    base_url: &str,
    wire_api: WireApi,
    cap_partial: bool,
) -> Result<(TestCodex, MockServer)> {
    let server = MockServer::start().await;
    let provider = output_cap_provider(base_url, wire_api);
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
            if cap_partial {
                config
                    .features
                    .enable(codex_features::Feature::CapPartialDiagnostics)
                    .expect("enable cap partial diagnostics");
            }
        })
        .build_with_auto_env(&server)
        .await?;
    Ok((test, server))
}

fn chat_chunk(delta: Value, finish_reason: Option<&str>) -> Value {
    let mut choice = json!({"index": 0, "delta": delta});
    choice["finish_reason"] = finish_reason.map(Value::from).unwrap_or(Value::Null);
    json!({
        "id": "chatcmpl-cap", "object": "chat.completion.chunk", "created": 1,
        "model": "cap-test", "choices": [choice],
    })
}

fn sse(frames: &[Value]) -> String {
    let mut body = String::new();
    for frame in frames {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    body
}

/// A truncated Chat tool call: arguments cut mid-JSON, no terminal of its own.
fn truncated_chat_tool_call() -> Value {
    json!({"tool_calls": [{
        "index": 0, "id": "call_cap_truncated", "type": "function",
        "function": {"name": "shell", "arguments": "{\"command\":[\"ech"}
    }]})
}

/// The Chat wire spells the output cap `max_tokens`, but endpoints that
/// reject the legacy field (reasoning models, `modern_output_cap`) get it
/// renamed to `max_completion_tokens` by the wire client. Exactly one of the
/// two must carry the budget.
fn assert_chat_wire_cap(body: &Value) {
    let legacy = body.get("max_tokens");
    let modern = body.get("max_completion_tokens");
    match (legacy, modern) {
        (Some(value), None) | (None, Some(value)) => {
            assert_eq!(value, &json!(64), "the single chat cap field is the budget")
        }
        (Some(_), Some(_)) => panic!("both chat cap fields present: {body}"),
        (None, None) => panic!("no chat cap field reached the wire: {body}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_output_cap_error_is_terminal_without_core_resampling() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let mut terminal = chat_chunk(json!({}), Some("length"));
    terminal["usage"] = json!({"prompt_tokens": 4, "completion_tokens": 64, "total_tokens": 68});
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(truncated_chat_tool_call(), None),
        terminal,
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Chat).await?;
    let outcome = run_turn_until_complete(&test, "hit the chat cap").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    assert_chat_wire_cap(&log.first_body());

    // The failed sampling's terminal-frame usage still enters the durable
    // ledger, under its own host key.
    let records = persisted_usage_records(&test);
    assert_eq!(records.len(), 1, "one capped sampling, one usage record");
    let record = &records[0];
    let key = record
        .response_key
        .as_deref()
        .expect("capped sampling carries its host key");
    assert!(key.starts_with("sampling:"), "host key shape: {key}");
    assert_eq!(record.response_id, "chatcmpl-cap");
    assert_eq!(
        (
            record.usage.input_tokens,
            record.usage.output_tokens,
            record.usage.total_tokens
        ),
        (4, 64, 68),
        "the terminal frame's reported counters survive the error mapping"
    );
    assert_eq!(record.thread_token_usage.total_tokens, 68);
    Ok(())
}

/// With CapPartialDiagnostics OFF (the default), a capped turn must not
/// emit the event or persist the rollout line: rollouts stay readable by
/// readers that predate the `cap_partial` variant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_partial_stays_absent_when_the_feature_is_off() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Chat).await?;
    let outcome = run_turn_until_complete(&test, "cap without diagnostics").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    assert!(
        outcome.cap_partials.is_empty(),
        "the default configuration must not emit CapPartial events"
    );
    assert!(
        rollout_lines(&test).is_empty(),
        "no cap_partial line may be persisted by default"
    );
    Ok(())
}

/// With the feature ON, the capped sampling emits ONE bounded CapPartial
/// before the failure terminal, persists it in the rollout with the same
/// host sampling key as its usage record, and the rollout still resumes
/// and serves a follow-up turn.
// The default (omitted) mode is the store's paginated default; the legacy
// case pins the same contract for legacy rollouts.
#[test_case::test_case(None; "default")]
#[test_case::test_case(Some(codex_protocol::protocol::ThreadHistoryMode::Legacy); "legacy")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_partial_emits_persists_and_the_rollout_resumes(
    history_mode: Option<codex_protocol::protocol::ThreadHistoryMode>,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let mut terminal = chat_chunk(json!({}), Some("length"));
    terminal["usage"] = json!({"prompt_tokens": 6, "completion_tokens": 64, "total_tokens": 70});
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(truncated_chat_tool_call(), None),
        terminal,
    ]) + "data: [DONE]\n\n";
    let log = RequestLog::default();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .and(log.clone())
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(body, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // The follow-up turn after resume gets a normal completion. The resumed
    // instance runs the default Responses provider, so the recovery mock
    // mounts on the Responses path.
    let recovered = responses::sse(vec![
        responses::ev_response_created("resp_recovered"),
        responses::ev_message_item_added("msg_recovered", ""),
        responses::ev_output_text_delta("recovered"),
        responses::ev_completed("resp_recovered"),
    ]);
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(recovered, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider_base = server.uri();
    let provider = output_cap_provider(&provider_base, WireApi::Chat);
    let mut builder = test_codex();
    if let Some(history_mode) = history_mode {
        builder = builder.with_history_mode(history_mode);
    }
    let test = builder
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
            config
                .features
                .enable(codex_features::Feature::CapPartialDiagnostics)
                .expect("enable cap partial diagnostics");
        })
        .build_with_auto_env(&server)
        .await?;
    let outcome = run_turn_until_complete(&test, "cap with diagnostics").await?;
    assert_cap_terminal(&outcome, 1, "partial cap answer");

    // Exactly one bounded diagnostic, ordered strictly before the terminal.
    assert_eq!(outcome.cap_partials.len(), 1, "one CapPartial event");
    let partial = &outcome.cap_partials[0];
    let index_of = |kind: &str| {
        outcome
            .terminal_order
            .iter()
            .position(|entry| *entry == kind)
    };
    let (partial_at, error_at) = (
        index_of("cap_partial").expect("cap partial ordered"),
        index_of("error").expect("error ordered"),
    );
    assert!(partial_at < error_at, "diagnostic precedes the failure");
    let assistant = partial
        .fragments
        .iter()
        .find(|fragment| {
            fragment.kind == codex_protocol::protocol::CapPartialFragmentKind::AssistantText
        })
        .expect("assistant text captured");
    assert_eq!(assistant.text, "partial cap answer");
    assert!(!assistant.truncated);
    assert!(
        !partial.fragments.iter().any(|fragment| {
            fragment.kind == codex_protocol::protocol::CapPartialFragmentKind::ToolArguments
        }),
        "the rig bridge buffers tool-call events until completion; a cap never          delivers them to the turn loop's observation point"
    );
    assert_eq!(
        partial.budget.token_evidence,
        codex_protocol::protocol::TokenBudgetEvidence::Unverified
    );
    let reported = partial
        .reported_usage
        .as_ref()
        .expect("terminal usage rides the partial");
    assert_eq!(
        (
            reported.counts.input_tokens,
            reported.counts.output_tokens,
            reported.counts.total_tokens
        ),
        (Some(6), Some(64), Some(70))
    );

    // The persisted usage record and the persisted partial share the failed
    // sampling's host key, and the partial line round-trips through the
    // rollout decoder.
    let usage_records = persisted_usage_records(&test);
    let persisted = rollout_lines(&test);
    assert_eq!(persisted.len(), 1, "exactly one persisted cap_partial line");
    match &persisted[0] {
        codex_history::RolloutItem::EventMsg(EventMsg::CapPartial(event)) => {
            assert_eq!(event.response_key, partial.response_key);
            let usage_key = usage_records
                .first()
                .and_then(|record| record.response_key.clone());
            assert_eq!(
                Some(&event.response_key),
                usage_key.as_ref(),
                "usage and partial identify the same sampling"
            );
            assert_eq!(event.fragments, partial.fragments);
        }
        other => panic!("expected a persisted CapPartial line, got {other:?}"),
    }

    // Resume reads the rollout with the cap_partial line and serves a
    // follow-up turn normally.
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let home = test.home.clone();
    test.codex.shutdown_and_wait().await?;
    let resumed = test_codex()
        .resume(&server, home.clone(), rollout_path.clone())
        .await?;
    let outcome = run_turn_until_complete(&resumed, "follow up after resume").await?;
    assert!(
        outcome.turn_error.is_none(),
        "the follow-up turn completes: {:?}",
        outcome.turn_error
    );
    assert_eq!(outcome.agent_deltas, "recovered");
    assert!(
        outcome.cap_partials.is_empty(),
        "a normal completion emits no partial"
    );

    // Crash tolerance: a writer killed mid-line leaves a truncated tail.
    // Append one, then resume a SECOND time from the same rollout — the
    // reader must tolerate the partial line and serve another turn.
    resumed.codex.shutdown_and_wait().await?;
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout_path)
            .expect("open rollout for crash tail");
        file.write_all(b"{\"timestamp\":\"2026-10-11T00:00:00.000Z\",\"ordi")
            .expect("append truncated crash line");
        file.flush().expect("flush crash tail");
    }
    let twice_resumed = test_codex()
        .resume(&server, home, rollout_path.clone())
        .await?;
    let outcome = run_turn_until_complete(&twice_resumed, "second resume").await?;
    assert!(
        outcome.turn_error.is_none(),
        "the twice-resumed turn completes: {:?}",
        outcome.turn_error
    );
    assert_eq!(outcome.agent_deltas, "recovered");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_partial_keeps_text_seeded_by_output_item_added() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = responses::sse(vec![
        responses::ev_response_created("seeded-cap"),
        responses::ev_message_item_added("seeded-message", "initial prefix"),
        responses::ev_output_text_delta(" and tail"),
        json!({
            "type": "response.incomplete",
            "response": {"id": "seeded-cap", "incomplete_details": {"reason": "max_output_tokens"}}
        }),
    ]);
    let log = mount_capturing_response(&server, "/v1/responses", body).await?;
    let (test, _env) =
        cap_test_instance_with(&server.uri(), WireApi::Responses, /*cap_partial*/ true).await?;
    let outcome = run_turn_until_complete(&test, "cap after a seeded message").await?;
    assert_cap_terminal(&outcome, log.posts(), " and tail");
    assert_eq!(outcome.cap_partials.len(), 1);
    assert_eq!(
        outcome.cap_partials[0]
            .fragments
            .iter()
            .map(|fragment| (
                fragment.kind,
                fragment.fragment_key.as_str(),
                fragment.text.as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![(
            codex_protocol::protocol::CapPartialFragmentKind::AssistantText,
            "seeded-message",
            "initial prefix and tail",
        )]
    );
    Ok(())
}

/// Both Responses routes dispatch a complete tool item received before the
/// cap terminal. Capturing its arguments as diagnostics must preserve that
/// execution and must not execute the tool a second time.
#[test_case::test_case(None; "rig")]
#[test_case::test_case(Some(codex_model_provider_info::ChatBridge::Native); "native")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_cap_partial_captures_completed_tool_arguments_as_diagnostics(
    bridge: Option<codex_model_provider_info::ChatBridge>,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = responses::sse(vec![
        responses::ev_response_created("resp_cap_diag"),
        responses::ev_message_item_added("msg_diag", ""),
        responses::ev_output_text_delta("partial with a tool"),
        responses::ev_exec_command_call("call_diag", "echo hi"),
        json!({
            "type": "response.incomplete",
            "response": {"id": "resp_cap_diag", "incomplete_details": {"reason": "max_output_tokens"}}
        }),
    ]);
    let log = mount_capturing_response(&server, "/v1/responses", body).await?;
    let mut provider = output_cap_provider(&server.uri(), WireApi::Responses);
    provider.experimental_bridge = bridge;
    let test = test_codex()
        .with_model("gpt-5.5")
        .with_model_info_override("gpt-5.5", |model| {
            model.shell_type = codex_protocol::openai_models::ConfigShellToolType::UnifiedExec;
            model.tool_mode = Some(codex_protocol::openai_models::ToolMode::Direct);
        })
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
            for feature in [
                codex_features::Feature::ShellTool,
                codex_features::Feature::UnifiedExec,
                codex_features::Feature::CapPartialDiagnostics,
            ] {
                config
                    .features
                    .enable(feature)
                    .expect("enable tool diagnostics");
            }
            config
                .permissions
                .set_permission_profile(codex_protocol::models::PermissionProfile::Disabled)
                .expect("test command permissions");
        })
        .build_with_auto_env(&server)
        .await?;
    let outcome = run_turn_until_complete(&test, "cap after a whole tool call").await?;
    let terminal = outcome
        .turn_error
        .as_deref()
        .context("cap must fail the turn")?;
    assert!(terminal.contains("Output token limit reached"));
    assert_eq!(outcome.errors.len(), 1, "one cap terminal error event");
    assert!(outcome.errors[0].contains("Output token limit reached"));
    assert_eq!(log.posts(), 1, "the cap must not resample");
    assert_eq!(
        (outcome.completed_agent_items, outcome.completed_responses),
        (0, 0),
        "the cap must not complete the assistant item or response"
    );
    assert_eq!(outcome.final_agent_message, None);
    assert_eq!(outcome.agent_deltas, "partial with a tool");
    assert_eq!(
        outcome.exec_begun, 1,
        "the whole tool executes exactly once"
    );
    assert!(
        log.first_body()["tools"]
            .as_array()
            .context("the request advertises direct tools")?
            .iter()
            .any(|tool| tool["name"] == "exec_command"),
        "exec_command must be registered on the native wire"
    );
    let rollout_path = test.codex.rollout_path().context("rollout path")?;
    let outputs = std::fs::read_to_string(rollout_path)?
        .lines()
        .filter_map(|line| codex_rollout::parse_rollout_line(line).ok())
        .filter_map(|line| match line.item {
            codex_history::RolloutItem::ResponseItem(envelope) => match envelope.item {
                codex_protocol::models::ResponseItem::FunctionCallOutput {
                    call_id,
                    output,
                    ..
                } if call_id.as_deref() == Some("call_diag") => Some(output),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        outputs.len(),
        1,
        "diagnostics must not duplicate tool execution"
    );
    let output = outputs[0]
        .text_content()
        .context("exec_command text output")?;
    assert!(output.contains("Process exited with code 0"));
    assert!(output.lines().any(|line| line.trim() == "hi"));
    assert_eq!(outcome.cap_partials.len(), 1);
    let partial = &outcome.cap_partials[0];
    let tool = partial
        .fragments
        .iter()
        .find(|fragment| {
            fragment.kind == codex_protocol::protocol::CapPartialFragmentKind::ToolArguments
        })
        .expect("the whole tool call captured as diagnostics");
    assert_eq!(tool.fragment_key, "call_diag");
    assert_eq!(tool.text, "call exec_command: {\"cmd\":\"echo hi\"}");
    assert!(
        partial
            .fragments
            .iter()
            .any(|fragment| fragment.text == "partial with a tool"),
        "assistant text captured alongside"
    );
    Ok(())
}

/// The persisted `cap_partial` EventMsg line uses the `type` tag every
/// EventMsg uses; a reader built before the variant cannot decode the line.
/// Default-off emission (see cap_partial_stays_absent_when_the_feature_is_off)
/// is what protects such readers; this test pins the wire shape itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_partial_rollout_line_has_the_eventmsg_wire_shape() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) = cap_test_instance_with(&server.uri(), WireApi::Chat, true).await?;
    let outcome = run_turn_until_complete(&test, "wire shape").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    let path = test.codex.rollout_path().expect("rollout path");
    let raw = std::fs::read_to_string(&path).expect("read rollout");
    let line = raw
        .lines()
        .find(|line| line.contains("cap_partial"))
        .expect("a cap_partial line")
        .trim()
        .to_string();
    let value: Value = serde_json::from_str(&line).expect("line is JSON");
    assert_eq!(value["type"], "event_msg", "rollout item tag");
    assert_eq!(
        value["payload"]["type"], "cap_partial",
        "internal EventMsg tag"
    );
    // An EventMsg decoder that lacks the variant fails exactly here; the
    // materialization path treats that as an unknown line and skips it.
    let legacy_shape: Result<serde_json::Value, _> =
        serde_json::from_str::<serde_json::Value>(&line.replace("cap_partial", "capPartialX"));
    assert!(legacy_shape.is_ok(), "value-level parsing stays generic");
    Ok(())
}

/// Reads the persisted CapPartial rollout items of a test instance.
fn rollout_lines(test: &TestCodex) -> Vec<codex_history::RolloutItem> {
    let path = test.codex.rollout_path().expect("rollout path");
    std::fs::read_to_string(&path)
        .expect("read rollout")
        .lines()
        .filter_map(|line| codex_rollout::parse_rollout_line(line).ok())
        .filter_map(|line| match line.item {
            codex_history::RolloutItem::EventMsg(EventMsg::CapPartial(event)) => Some(
                codex_history::RolloutItem::EventMsg(EventMsg::CapPartial(event)),
            ),
            _ => None,
        })
        .collect()
}

/// Reads the durable TokenUsageRecord lines of a test instance's rollout.
fn persisted_usage_records(test: &TestCodex) -> Vec<codex_protocol::protocol::TokenUsageRecord> {
    let path = test.codex.rollout_path().expect("rollout path");
    std::fs::read_to_string(&path)
        .expect("read rollout")
        .lines()
        .filter_map(|line| codex_rollout::parse_rollout_line(line).ok())
        .filter_map(|line| match line.item {
            codex_history::RolloutItem::TokenUsageRecord(record) => Some(record),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_cap_does_not_execute_a_tool_with_valid_json_arguments() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(
            json!({"tool_calls": [{
                "index": 0, "id": "call_cap_valid", "type": "function",
                "function": {"name": "shell", "arguments": "{\"command\":[\"echo\",\"hi\"]}"}
            }]}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Chat).await?;
    let outcome = run_turn_until_complete(&test, "cap after valid tool arguments").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_cap_error_is_terminal_without_core_resampling() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = format!(
        "{}{}{}{}{}{}{}{}",
        anthropic_frame(
            "message_start",
            json!({"message": {"id": "msg_cap", "type": "message", "role": "assistant", "content": [], "model": "cap-test", "stop_reason": null, "usage": {"input_tokens": 4, "output_tokens": 0}}})
        ),
        anthropic_frame(
            "content_block_start",
            json!({"index": 0, "content_block": {"type": "text", "text": ""}})
        ),
        anthropic_frame(
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": "partial cap answer"}})
        ),
        anthropic_frame("content_block_stop", json!({"index": 0})),
        anthropic_frame(
            "content_block_start",
            json!({"index": 1, "content_block": {"type": "tool_use", "id": "toolu_cap_truncated", "name": "shell", "input": {}}})
        ),
        anthropic_frame(
            "content_block_delta",
            json!({"index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"command\":[\"echo\",\"hi\"]}"}})
        ),
        // Even a confirmed, valid tool block must not execute when the
        // enclosing response exhausts its output cap.
        anthropic_frame("content_block_stop", json!({"index": 1})),
        anthropic_frame(
            "message_delta",
            json!({"delta": {"stop_reason": "max_tokens", "stop_sequence": null}, "usage": {"output_tokens": 64}})
        ),
    ) + "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let log = mount_capturing_response(&server, "/v1/messages", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Anthropic).await?;
    let outcome = run_turn_until_complete(&test, "hit the anthropic cap").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    assert_eq!(log.first_body()["max_tokens"], json!(64));
    Ok(())
}

/// Shared builder for the Anthropic cap-usage matrix: message_start and the
/// terminal message_delta carry exactly the usage objects under test.
async fn anthropic_cap_usage_matrix_case(
    start_usage: Option<Value>,
    delta_usage: Option<Value>,
) -> Result<(TurnOutcome, Vec<TokenUsageRecord>, usize)> {
    let server = MockServer::start().await;
    let mut message = json!({
        "id": "msg_cap", "type": "message", "role": "assistant",
        "content": [], "model": "cap-test", "stop_reason": null,
    });
    if let Some(usage) = start_usage {
        message["usage"] = usage;
    }
    let mut delta = json!({"delta": {"stop_reason": "max_tokens", "stop_sequence": null}});
    if let Some(usage) = delta_usage {
        delta["usage"] = usage;
    }
    let body = format!(
        "{}{}{}{}",
        anthropic_frame("message_start", json!({"message": message})),
        anthropic_frame(
            "content_block_start",
            json!({"index": 0, "content_block": {"type": "text", "text": ""}})
        ),
        anthropic_frame(
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": "partial cap answer"}})
        ),
        anthropic_frame("message_delta", delta),
    ) + "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let log = mount_capturing_response(&server, "/v1/messages", body).await?;
    let (test, _env) =
        cap_test_instance_with(&server.uri(), WireApi::Anthropic, /*cap_partial*/ true).await?;
    let outcome = run_turn_until_complete(&test, "anthropic cap usage matrix").await?;
    let records = persisted_usage_records(&test);
    Ok((outcome, records, log.posts()))
}

/// Start/delta usage accumulates: the start frame's input count and the
/// terminal delta's output count both survive; the wire never reports a
/// total so the diagnostic stays Incomplete with no total.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cap_usage_accumulates_start_and_delta() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) = anthropic_cap_usage_matrix_case(
        Some(json!({"input_tokens": 4, "output_tokens": 0})),
        Some(json!({"output_tokens": 64})),
    )
    .await?;
    assert_cap_terminal(&outcome, posts, "partial cap answer");
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0].usage.input_tokens,
            records[0].usage.output_tokens,
            records[0].usage.total_tokens,
        ),
        (4, 64, 0),
        "the start frame's input and the delta's output survive; the absent total is zero"
    );
    let reported = outcome.cap_partials[0]
        .reported_usage
        .as_ref()
        .expect("the accumulated report rides the diagnostic");
    assert_eq!(
        (
            reported.counts.input_tokens,
            reported.counts.output_tokens,
            reported.counts.total_tokens,
        ),
        (Some(4), Some(64), None),
        "presence keeps the unreported total absent"
    );
    assert_eq!(
        reported.completeness,
        codex_protocol::protocol::UsageCompleteness::Incomplete
    );
    Ok(())
}

/// Rig's Anthropic deserializer requires a usage object on both
/// message_start and the terminal message_delta (a stream omitting either
/// is unparseable and fails as a retryable stream error, covered by
/// anthropic_cap_missing_usage_is_retryable). Another legal matrix cell is an
/// explicit zero on the terminal delta: rig 0.42 normalizes a zero output
/// count to missing, and the wire accumulator must restore it as a
/// reported zero end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cap_terminal_delta_explicit_zero_output_is_reported() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) = anthropic_cap_usage_matrix_case(
        Some(json!({"input_tokens": 4, "output_tokens": 0})),
        Some(json!({"output_tokens": 0})),
    )
    .await?;
    assert_cap_terminal(&outcome, posts, "partial cap answer");
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0].usage.input_tokens,
            records[0].usage.output_tokens
        ),
        (4, 0),
        "the explicit zero output rides the ledger as a count, not as missing"
    );
    let reported = outcome.cap_partials[0]
        .reported_usage
        .as_ref()
        .expect("the wire-observed report rides the diagnostic");
    assert_eq!(
        (reported.counts.input_tokens, reported.counts.output_tokens),
        (Some(4), Some(0)),
        "the explicit zero output stays a reported zero despite rig's zero-to-missing normalization"
    );
    assert_eq!(
        reported.counts.total_tokens, None,
        "the Anthropic wire never reports a total"
    );
    Ok(())
}

/// Semantic control for the shared terminal policy: the native Responses
/// decoder classifies `incomplete_details.reason=max_output_tokens` the same
/// non-retryable way, on the fork's Responses passthrough wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_output_cap_error_is_terminal_without_core_resampling() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = responses::sse(vec![
        responses::ev_response_created("resp_cap"),
        responses::ev_message_item_added("msg_cap", ""),
        responses::ev_output_text_delta("partial cap answer"),
        json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_cap",
                "incomplete_details": {"reason": "max_output_tokens"},
                "usage": {"input_tokens": 4, "output_tokens": 64, "total_tokens": 68}
            }
        }),
    ]);
    let log = mount_capturing_response(&server, "/v1/responses", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Responses).await?;
    let outcome = run_turn_until_complete(&test, "hit the responses cap").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    assert_eq!(log.first_body()["max_output_tokens"], json!(64));
    // The native decoder's terminal-frame usage enters the same durable
    // ledger the Chat wire uses.
    let records = persisted_usage_records(&test);
    assert_eq!(records.len(), 1, "the capped sampling accounts its usage");
    assert!(
        records[0]
            .response_key
            .as_deref()
            .is_some_and(|key| key.starts_with("sampling:"))
    );
    assert_eq!(
        (
            records[0].usage.input_tokens,
            records[0].usage.output_tokens,
            records[0].usage.total_tokens
        ),
        (4, 64, 68),
        "the native terminal frame's counters survive the error mapping"
    );
    Ok(())
}

/// Shared builder for the Responses cap-usage presence matrix: one terminal
/// frame whose `usage` object carries exactly the counters under test.
async fn responses_cap_usage_matrix_case(
    usage: Option<Value>,
) -> Result<(TurnOutcome, Vec<TokenUsageRecord>, usize)> {
    let server = MockServer::start().await;
    let mut response = json!({
        "id": "resp_cap",
        "incomplete_details": {"reason": "max_output_tokens"},
    });
    if let Some(usage) = usage {
        response["usage"] = usage;
    }
    let body = responses::sse(vec![
        responses::ev_response_created("resp_cap"),
        responses::ev_message_item_added("msg_cap", ""),
        responses::ev_output_text_delta("partial cap answer"),
        json!({"type": "response.incomplete", "response": response}),
    ]);
    let log = mount_capturing_response(&server, "/v1/responses", body).await?;
    let (test, _env) =
        cap_test_instance_with(&server.uri(), WireApi::Responses, /*cap_partial*/ true).await?;
    let outcome = run_turn_until_complete(&test, "responses cap usage matrix").await?;
    let records = persisted_usage_records(&test);
    Ok((outcome, records, log.posts()))
}

/// A cap terminal frame with NO usage object must not fabricate counts: no
/// ledger record is written and the diagnostic carries no report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_cap_without_terminal_usage_records_nothing() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) = responses_cap_usage_matrix_case(None).await?;
    assert_cap_terminal(&outcome, posts, "partial cap answer");
    assert!(
        records.is_empty(),
        "an absent usage object must not fabricate a zero record"
    );
    assert!(
        outcome.cap_partials[0].reported_usage.is_none(),
        "the diagnostic carries no usage either"
    );
    Ok(())
}

/// A partially reported usage object keeps per-counter presence: the ledger
/// stores the reported counter with zeros for absent ones, while the
/// presence-preserving diagnostic marks the report Incomplete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_cap_partial_terminal_usage_keeps_presence() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) =
        responses_cap_usage_matrix_case(Some(json!({"input_tokens": 4}))).await?;
    assert_cap_terminal(&outcome, posts, "partial cap answer");
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0].usage.input_tokens,
            records[0].usage.output_tokens,
            records[0].usage.total_tokens
        ),
        (4, 0, 0),
        "absent counters are zero in the ledger (it has no presence)"
    );
    let reported = outcome.cap_partials[0]
        .reported_usage
        .as_ref()
        .expect("the partial report rides the diagnostic");
    assert_eq!(reported.counts.input_tokens, Some(4));
    assert_eq!(reported.counts.output_tokens, None);
    assert_eq!(
        reported.completeness,
        codex_protocol::protocol::UsageCompleteness::Incomplete
    );
    Ok(())
}

/// Explicit zeros in the terminal usage are reported counts, not absence:
/// they survive into the ledger and the diagnostic alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_cap_explicit_zero_usage_is_reported() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) = responses_cap_usage_matrix_case(Some(json!({
        "input_tokens": 4, "output_tokens": 0, "total_tokens": 4
    })))
    .await?;
    assert_cap_terminal(&outcome, posts, "partial cap answer");
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0].usage.input_tokens,
            records[0].usage.output_tokens
        ),
        (4, 0)
    );
    let reported = outcome.cap_partials[0]
        .reported_usage
        .as_ref()
        .expect("the partial report rides the diagnostic");
    assert_eq!(
        reported.counts.output_tokens,
        Some(0),
        "an explicit zero is a reported counter, never dropped to absent"
    );
    Ok(())
}

async fn chat_cap_usage_matrix_case(
    usage: Value,
) -> Result<(TurnOutcome, Vec<TokenUsageRecord>, usize)> {
    let server = MockServer::start().await;
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
        json!({
            "id": "chatcmpl-cap", "object": "chat.completion.chunk", "created": 1,
            "model": "cap-test", "choices": [], "usage": usage,
        }),
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) =
        cap_test_instance_with(&server.uri(), WireApi::Chat, /*cap_partial*/ true).await?;
    let outcome = run_turn_until_complete(&test, "chat cap usage matrix").await?;
    let records = persisted_usage_records(&test);
    Ok((outcome, records, log.posts()))
}

/// Rig rejects malformed required counters before publishing Final. Its
/// stream error is retryable; neither wire-observed counters nor the cap
/// marker may become a completed response, cap diagnostic or ledger record.
fn assert_retryable_usage_failure(
    outcome: &TurnOutcome,
    records: &[TokenUsageRecord],
    posts: usize,
    error_fragment: &str,
) {
    assert_eq!(
        posts, 3,
        "one sampling plus the configured two stream retries"
    );
    let terminal = outcome.turn_error.as_deref().expect("a stream failure");
    assert!(
        terminal.contains(error_fragment),
        "unexpected terminal: {terminal}"
    );
    assert!(
        !terminal.contains("Output token limit reached"),
        "an undecodable terminal must not be classified as a cap: {terminal}"
    );
    assert_eq!(outcome.errors.len(), 1, "exactly one terminal error event");
    assert_eq!(outcome.final_agent_message, None);
    assert_eq!(
        (
            outcome.completed_agent_items,
            outcome.completed_responses,
            outcome.exec_begun
        ),
        (0, 0, 0)
    );
    assert!(outcome.completed_tool_items.is_empty());
    assert!(outcome.cap_partials.is_empty());
    assert!(
        records.is_empty(),
        "no terminal usage was accepted: {records:?}"
    );
}

/// Unlike the raw accumulator, Rig's unsigned Chat/Anthropic usage
/// deserializers reject negative counters. Responses extracts each counter
/// independently, so its legal counters survive a delivered cap terminal.
#[test_case::test_case(WireApi::Chat; "chat")]
#[test_case::test_case(WireApi::Anthropic; "anthropic")]
#[test_case::test_case(WireApi::Responses; "responses")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_negative_usage_follows_the_protocol_decoder(wire_api: WireApi) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) = match wire_api {
        WireApi::Chat => {
            chat_cap_usage_matrix_case(json!({
                "prompt_tokens": -1, "completion_tokens": 64, "total_tokens": 64,
            }))
            .await?
        }
        WireApi::Anthropic => {
            anthropic_cap_usage_matrix_case(
                Some(json!({"input_tokens": 4, "output_tokens": 0})),
                Some(json!({"input_tokens": -1, "output_tokens": 64})),
            )
            .await?
        }
        WireApi::Responses => {
            responses_cap_usage_matrix_case(Some(json!({
                "input_tokens": -1, "output_tokens": 64, "total_tokens": 64,
            })))
            .await?
        }
    };
    match wire_api {
        WireApi::Chat | WireApi::Anthropic => {
            assert_retryable_usage_failure(&outcome, &records, posts, "invalid value: integer");
        }
        WireApi::Responses => {
            assert_cap_terminal(&outcome, posts, "partial cap answer");
            assert_eq!(records.len(), 1);
            assert_eq!(
                records[0].usage,
                TokenUsage {
                    output_tokens: 64,
                    total_tokens: 64,
                    ..Default::default()
                }
            );
            assert_eq!(
                outcome.cap_partials[0].reported_usage,
                ReportedResponseUsage::from_counters(ReportedUsageCounters {
                    output_tokens: Some(64),
                    total_tokens: Some(64),
                    ..Default::default()
                })
            );
        }
    }
    Ok(())
}

/// A counter above i64 but inside the wire's unsigned range is parseable.
/// The cap report discards only that counter, retaining valid counts and
/// ending after one request rather than treating overflow as a retry.
#[test_case::test_case(WireApi::Chat; "chat")]
#[test_case::test_case(WireApi::Anthropic; "anthropic")]
#[test_case::test_case(WireApi::Responses; "responses")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_overflow_usage_keeps_other_reported_counters(wire_api: WireApi) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let (outcome, records, posts) = match wire_api {
        WireApi::Chat => {
            chat_cap_usage_matrix_case(json!({
                "prompt_tokens": 4, "completion_tokens": u64::MAX, "total_tokens": 4,
            }))
            .await?
        }
        WireApi::Anthropic => {
            anthropic_cap_usage_matrix_case(
                Some(json!({"input_tokens": 4, "output_tokens": 0})),
                Some(json!({"output_tokens": u64::MAX})),
            )
            .await?
        }
        WireApi::Responses => {
            responses_cap_usage_matrix_case(Some(json!({
                "input_tokens": 4, "output_tokens": u64::MAX, "total_tokens": 4,
            })))
            .await?
        }
    };
    assert_cap_terminal(&outcome, posts, "partial cap answer");
    assert_eq!(records.len(), 1);
    let total_tokens = match wire_api {
        WireApi::Anthropic => None,
        WireApi::Chat | WireApi::Responses => Some(4),
    };
    assert_eq!(
        records[0].usage,
        TokenUsage {
            input_tokens: 4,
            total_tokens: total_tokens.unwrap_or_default(),
            ..Default::default()
        }
    );
    assert_eq!(
        outcome.cap_partials[0].reported_usage,
        ReportedResponseUsage::from_counters(ReportedUsageCounters {
            input_tokens: Some(4),
            total_tokens,
            ..Default::default()
        })
    );
    Ok(())
}

#[test_case::test_case(None, Some(json!({"output_tokens": 64})); "message_start")]
#[test_case::test_case(Some(json!({"input_tokens": 4, "output_tokens": 0})), None; "message_delta")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cap_missing_usage_is_retryable(
    start_usage: Option<Value>,
    delta_usage: Option<Value>,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    // message_stop remains present: this exercises required usage decoding,
    // independently of the existing missing-message_stop truncation case.
    let (outcome, records, posts) =
        anthropic_cap_usage_matrix_case(start_usage, delta_usage).await?;
    assert_retryable_usage_failure(&outcome, &records, posts, "missing field `usage`");
    Ok(())
}

/// A `finish_reason=stop` frame arriving AFTER the length terminal must not
/// convert the failed turn into a success or trigger a resample.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_cap_terminal_error_is_not_overwritten_by_late_completed_frame() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
        chat_chunk(json!({"content": "late smuggled success"}), Some("stop")),
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Chat).await?;
    let outcome = run_turn_until_complete(&test, "late completed after cap").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    assert_chat_wire_cap(&log.first_body());
    Ok(())
}

fn anthropic_frame(event: &str, mut data: Value) -> String {
    if let Some(object) = data.as_object_mut() {
        object.insert("type".into(), Value::from(event));
    }
    format!("event: {event}\ndata: {data}\n\n")
}

/// What a raw capped Chat stream does after the finish_reason=length chunk.
enum RawChatAfter {
    /// Keep the connection open, sending nothing further.
    HoldOpen,
}

/// Minimal raw SSE gateway: reads one request, replies with `frames`, then
/// applies `after`. Returns the base URI and the captured request bodies.
/// HoldOpen replies chunked WITHOUT the terminating zero-chunk and keeps the
/// connection open, so the client is genuinely waiting for more wire bytes.
async fn raw_chat_gateway(
    frames: String,
    after: RawChatAfter,
) -> Result<(String, tokio::task::JoinHandle<Vec<Value>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("gateway connection");
        let request = read_http_request(&mut socket).await.expect("read request");
        match after {
            RawChatAfter::HoldOpen => {
                let response = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write head");
                let chunk = format!("{:x}\r\n{}\r\n", frames.len(), frames);
                socket
                    .write_all(chunk.as_bytes())
                    .await
                    .expect("write chunk");
                socket.flush().await.expect("flush chunk");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
        request
    });
    Ok((format!("http://{address}"), handle))
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Result<Vec<Value>> {
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.context("read request")?;
        anyhow::ensure!(read != 0, "client closed before sending the request");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&data[..header_end]).context("request headers")?;
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim().to_string())
        })
        .context("request Content-Length")?
        .parse::<usize>()
        .context("parse request Content-Length")?;
    while data.len() < header_end + length {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.context("read request body")?;
        anyhow::ensure!(read != 0, "client closed mid-request");
        data.extend_from_slice(&chunk[..read]);
    }
    Ok(vec![
        serde_json::from_slice(&data[header_end..header_end + length]).context("body json")?,
    ])
}

/// Symmetric with the Anthropic boundary: a Chat wire that breaks after the
/// finish_reason chunk but before `[DONE]` has NOT delivered its terminal —
/// the stream is a retryable truncation, never a success and never a claim
/// of the cap error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_cap_finish_chunk_without_done_is_retryable_truncation() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
    ]);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
    let seen = std::sync::Arc::clone(&captured);
    let gateway = tokio::spawn(async move {
        let mut served = 0usize;
        loop {
            let (mut socket, _) = listener.accept().await.expect("gateway connection");
            let request = read_http_request(&mut socket).await.expect("read request");
            seen.lock().expect("record request").extend(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len(),
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write head");
            socket
                .write_all(body.as_bytes())
                .await
                .expect("write frames");
            socket.flush().await.expect("flush frames");
            socket.shutdown().await.expect("close without [DONE]");
            served += 1;
            if served == 3 {
                return served;
            }
        }
    });
    let (test, _env) = cap_test_instance(&format!("http://{address}"), WireApi::Chat).await?;
    let outcome = run_turn_until_complete(&test, "chat wire truncation").await?;
    let served = tokio::time::timeout(Duration::from_secs(10), gateway)
        .await
        .map_err(|_| anyhow::anyhow!("gateway did not observe the expected attempts"))?
        .expect("gateway join");
    let count = captured.lock().expect("captured").len();
    assert_eq!((served, count), (3, 3), "1 attempt + 2 stream retries");
    let terminal = outcome
        .turn_error
        .as_deref()
        .context("truncated wire must end in an error")?;
    assert!(
        terminal.contains("ended before its SSE terminal"),
        "the failure must be the truncation class, not a success or cap error: {terminal}"
    );
    assert!(
        !terminal.contains("Output token limit reached"),
        "an undelivered terminal must not claim the cap error: {terminal}"
    );
    assert_eq!(
        outcome.completed_tool_items,
        Vec::<String>::new(),
        "no tool may complete on a truncated wire"
    );
    assert_eq!(outcome.exec_begun, 0);
    Ok(())
}

/// V-D2-1: a usage-only chunk arriving AFTER the `finish_reason=length`
/// chunk but before `[DONE]` — the wire order several gateways use — must
/// still ride the cap terminal's presence-preserving report. The wire check
/// observes every frame that flows, and rig publishes its final record only
/// after `[DONE]`, so the snapshot the cap error carries includes the late
/// chunk; an explicit zero stays zero and unreported counters stay absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_cap_usage_only_chunk_after_length_finish_is_retained() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let late_usage = json!({
        "id": "chatcmpl-cap", "object": "chat.completion.chunk", "created": 1,
        "model": "cap-test", "choices": [],
        "usage": {
            "prompt_tokens": 6,
            "completion_tokens": 64,
            "prompt_tokens_details": {"cached_tokens": 0},
            "total_tokens": 70
        }
    });
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "late usage answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
        late_usage,
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) =
        cap_test_instance_with(&server.uri(), WireApi::Chat, /*cap_partial*/ true).await?;
    let outcome = run_turn_until_complete(&test, "chat late usage chunk").await?;
    assert_cap_terminal(&outcome, 1, "late usage answer");
    assert_eq!(log.posts(), 1, "no resample");
    let partial = &outcome.cap_partials[0];
    let reported = partial
        .reported_usage
        .as_ref()
        .expect("the late usage-only chunk rides the cap terminal");
    assert_eq!(
        (
            reported.counts.input_tokens,
            reported.counts.output_tokens,
            reported.counts.cached_input_tokens,
            reported.counts.total_tokens,
        ),
        (Some(6), Some(64), Some(0), Some(70)),
        "explicit zero presence is preserved, reported counters are not lost"
    );
    assert_eq!(
        (
            reported.counts.cache_write_input_tokens,
            reported.counts.reasoning_output_tokens
        ),
        (None, None),
        "the Chat wire reports neither counter; absence stays absent"
    );
    assert_eq!(
        reported.completeness,
        codex_protocol::protocol::UsageCompleteness::Incomplete,
        "without cache-write/reasoning counters the report is incomplete, not normalized"
    );
    Ok(())
}

/// A primary-choice delta arriving after the first Chat terminal chunk must
/// be dropped, not spliced into the visible partial: the first terminal is
/// the authoritative one, and content past it never reaches the turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_late_primary_chunk_after_length_finish_is_dropped() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "real partial"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
        chat_chunk(
            json!({"role": "assistant", "content": "ghost continuation"}),
            None,
        ),
    ]) + "data: [DONE]\n\n";
    let log = mount_capturing_response(&server, "/v1/chat/completions", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Chat).await?;
    let outcome = run_turn_until_complete(&test, "chat late primary chunk").await?;
    assert_cap_terminal(&outcome, 1, "real partial");
    assert_eq!(log.posts(), 1, "no resample");
    assert!(
        !outcome.agent_deltas.contains("ghost"),
        "content after the first terminal must never surface: {:?}",
        outcome.agent_deltas
    );
    Ok(())
}

/// Documented boundary: a Chat gateway that stalls after the finish chunk
/// (connection open, no `[DONE]`) has not delivered its terminal on the wire,
/// so the stream costs the idle budget and fails — it must NOT report
/// success or the cap error, and with stream retries 0 no resample follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_stalled_stream_after_length_chunk_costs_the_idle_budget() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let body = sse(&[
        chat_chunk(
            json!({"role": "assistant", "content": "partial cap answer"}),
            None,
        ),
        chat_chunk(json!({}), Some("length")),
    ]);
    let (base_url, gateway) = raw_chat_gateway(body, RawChatAfter::HoldOpen).await?;
    let (test, _env) = cap_test_instance(&base_url, WireApi::Chat).await?;
    // Stream retries stay 2 in the provider; the stalled stream must still
    // produce a bounded failure. The idle budget is 5s.
    // The provider's idle timeout is 5s; the stalled stream must not be
    // reported as success or as a delivered cap terminal, and must not hang
    // past the idle budget.
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(25),
        run_turn_until_complete(&test, "idle after cap terminal"),
    )
    .await
    .context("stalled stream must fail within the idle budget")??;
    let elapsed = started.elapsed();
    gateway.abort();
    assert!(
        outcome.final_agent_message.is_none(),
        "a stalled stream must not complete successfully"
    );
    let terminal = outcome
        .turn_error
        .as_deref()
        .context("a stalled stream must end in a turn error")?;
    assert!(
        !terminal.contains("Output token limit reached"),
        "an undelivered terminal must not claim the cap error: {terminal}"
    );
    assert!(
        elapsed >= Duration::from_millis(4500),
        "the idle budget must elapse (took {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_secs(25),
        "the failure must stay bounded by the idle budget (took {elapsed:?})"
    );
    assert_eq!(outcome.agent_deltas, "partial cap answer");
    Ok(())
}

/// The Anthropic wrapper holds the stop_reason terminal until `message_stop`
/// (truncation detection): a wire that dies after `stop_reason=max_tokens`
/// but before `message_stop` is a truncated stream, NOT a delivered cap
/// terminal — core may resample it. Pins the documented boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cap_stop_reason_without_message_stop_is_retryable_truncation() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let frames = format!(
        "{}{}{}{}",
        anthropic_frame(
            "message_start",
            json!({"message": {"id": "msg_cut", "type": "message", "role": "assistant", "content": [], "model": "cap-test", "stop_reason": null, "usage": {"input_tokens": 4, "output_tokens": 0}}})
        ),
        anthropic_frame(
            "content_block_start",
            json!({"index": 0, "content_block": {"type": "text", "text": ""}})
        ),
        anthropic_frame(
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": "cut short"}})
        ),
        anthropic_frame(
            "message_delta",
            json!({"delta": {"stop_reason": "max_tokens", "stop_sequence": null}, "usage": {"output_tokens": 64}})
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
    let seen = std::sync::Arc::clone(&captured);
    let gateway = tokio::spawn(async move {
        let mut served = 0usize;
        loop {
            let (mut socket, _) = listener.accept().await.expect("gateway connection");
            let request = read_http_request(&mut socket).await.expect("read request");
            seen.lock().expect("record request").extend(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                frames.len(),
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write head");
            socket
                .write_all(frames.as_bytes())
                .await
                .expect("write frames");
            socket.flush().await.expect("flush frames");
            socket.shutdown().await.expect("close without message_stop");
            served += 1;
            if served == 3 {
                return served;
            }
        }
    });
    let (test, _env) = cap_test_instance(&format!("http://{address}"), WireApi::Anthropic).await?;
    let outcome = run_turn_until_complete(&test, "anthropic wire truncation").await?;
    let served = tokio::time::timeout(Duration::from_secs(10), gateway)
        .await
        .map_err(|_| anyhow::anyhow!("gateway did not observe the expected attempts"))?
        .expect("gateway join");
    let count = captured.lock().expect("captured").len();
    assert_eq!((served, count), (3, 3), "1 attempt + 2 stream retries");
    let terminal = outcome
        .turn_error
        .as_deref()
        .context("truncated wire must end in an error")?;
    assert!(
        terminal.contains("ended before its SSE terminal"),
        "the failure must be the truncation class, not a success or cap error: {terminal}"
    );
    assert!(
        !terminal.contains("Output token limit reached"),
        "an undelivered terminal must not claim the cap error: {terminal}"
    );
    assert_eq!(
        outcome.agent_deltas, "cut shortcut shortcut short",
        "each resample streams its own partial deltas"
    );
    Ok(())
}
