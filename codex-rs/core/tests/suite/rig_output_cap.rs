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
    completed_tool_items: Vec<String>,
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
            EventMsg::Error(error) => outcome.errors.push(error.message),
            EventMsg::ExecCommandBegin(_) => outcome.exec_begun += 1,
            EventMsg::ItemCompleted(item) => {
                if let TurnItem::DynamicToolCall(call) = item.item {
                    outcome.completed_tool_items.push(call.tool);
                }
            }
            EventMsg::TurnComplete(complete) => {
                outcome.turn_error = complete.error.map(|error| error.message);
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
    // The cap terminal now flushes the already-streamed partial as a durable
    // history item, so a final AgentMessage may exist — but it must be
    // exactly the partial text (never a synthesized success), and the turn
    // still fails closed above.
    match outcome.final_agent_message.as_deref() {
        Some(message) => assert_eq!(
            message, partial_text,
            "a capped turn's final AgentMessage must be the flushed partial, not a success"
        ),
        None => {}
    }
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
    let server = MockServer::start().await;
    let provider = output_cap_provider(base_url, wire_api);
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
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
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_cap_error_is_terminal_without_core_resampling() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let body = format!(
        "{}{}{}{}{}{}{}",
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
            "response": {"id": "resp_cap", "incomplete_details": {"reason": "max_output_tokens"}}
        }),
    ]);
    let log = mount_capturing_response(&server, "/v1/responses", body).await?;
    let (test, _env) = cap_test_instance(&server.uri(), WireApi::Responses).await?;
    let outcome = run_turn_until_complete(&test, "hit the responses cap").await?;
    assert_cap_terminal(&outcome, log.posts(), "partial cap answer");
    assert_eq!(log.first_body()["max_output_tokens"], json!(64));
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
