//! Cross-wire cancellation contracts for the rig-bridged sampling loop.
//!
//! The Anthropic wire already pins `Op::Interrupt` behaviour end to end
//! (see `rig_anthropic_hosted_tools.rs`); these tests close the same
//! contract for the Chat and Responses passthrough wires:
//! - interrupting before response headers cancels the in-flight HTTP
//!   request (the server observes socket release) and the SAME core client
//!   immediately serves a follow-up turn;
//! - interrupting mid-stream after partial deltas has already been consumed
//!   resolves the turn as `TurnAborted` (never a fake `TurnComplete`), and
//!   no additional model attempt is paid despite non-zero retry budgets.
//!
//! Retry budgets are deliberately NON-zero: if core ever classified a
//! user interrupt as retryable, the "exactly two connections" assertion
//! would observe the extra attempt.
//!
//! Feature gate: run with
//! `just test -p codex-core --features rust-rig -E 'test(rig_interrupt)'`
//! and verify the selected count is non-zero.

#![cfg(feature = "rust-rig")]

use anyhow::Context;
use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use serde_json::Value;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use wiremock::MockServer;

fn interrupt_provider(base_url: &str, wire_api: WireApi) -> ModelProviderInfo {
    let protocol_name = match wire_api {
        WireApi::Chat => "chat",
        WireApi::Responses => "responses",
        WireApi::Anthropic => "anthropic",
    };
    ModelProviderInfo {
        capabilities: None,
        name: format!("rig-interrupt-{protocol_name}"),
        base_url: Some(format!("{base_url}/v1")),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        experimental_bridge: None,
        provider_id: Some("interrupt-test".into()),
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(2),
        stream_max_retries: Some(2),
        stream_idle_timeout_ms: Some(10_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        include_internal_metadata: false,
        max_output_tokens: Some(64),
        hosted_results_replay: None,
    }
}

/// One complete assistant-text SSE body in the wire's native shape
/// (cross-checked against the codex-rust-rig-bridge wire fixtures).
fn complete_sse(wire: WireApi, content: &str) -> String {
    match wire {
        WireApi::Chat => format!(
            concat!("data: {}\n\n", "data: {}\n\n", "data: [DONE]\n\n",),
            serde_json::json!({
                "id": "cc", "object": "chat.completion.chunk", "created": 1, "model": "m",
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": content}, "finish_reason": null}]
            }),
            serde_json::json!({
                "id": "cc", "object": "chat.completion.chunk", "created": 1, "model": "m",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 4, "completion_tokens": 1, "total_tokens": 5}
            }),
        ),
        WireApi::Responses => format!(
            concat!(
                "data: {}\n\n",
                "data: {}\n\n",
                "data: {}\n\n",
                "data: {}\n\n",
                "data: {}\n\n",
            ),
            serde_json::json!({"type": "response.created", "response": {"id": "r1", "model": "m"}}),
            serde_json::json!({"type": "response.output_item.added", "item": {"type": "message", "role": "assistant", "content": [], "id": "msg_1"}}),
            serde_json::json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": content}),
            serde_json::json!({"type": "response.output_item.done", "item": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": content}], "id": "msg_1"}}),
            serde_json::json!({"type": "response.completed", "response": {"id": "r1", "usage": {"input_tokens": 4, "output_tokens": 1, "total_tokens": 5}}}),
        ),
        WireApi::Anthropic => {
            unreachable!("anthropic interrupts live in rig_anthropic_hosted_tools.rs")
        }
    }
}

/// Response-frame prefix that proves the client consumed real deltas before
/// the interrupt lands.
fn partial_sse_prefix(wire: WireApi) -> String {
    match wire {
        WireApi::Chat => format!(
            "data: {}\n\ndata: {}\n\n",
            serde_json::json!({
                "id": "cc", "object": "chat.completion.chunk", "created": 1, "model": "m",
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}]
            }),
            serde_json::json!({
                "id": "cc", "object": "chat.completion.chunk", "created": 1, "model": "m",
                "choices": [{"index": 0, "delta": {"content": "partial delta about topics. "}, "finish_reason": null}]
            }),
        ),
        WireApi::Responses => format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\n",
            serde_json::json!({"type": "response.created", "response": {"id": "r1", "model": "m"}}),
            serde_json::json!({"type": "response.output_item.added", "item": {"type": "message", "role": "assistant", "content": [], "id": "msg_1"}}),
            serde_json::json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": "partial delta about topics. "}),
        ),
        WireApi::Anthropic => {
            unreachable!("anthropic interrupts live in rig_anthropic_hosted_tools.rs")
        }
    }
}

/// Read one full HTTP request (headers + content-length body) from the socket.
async fn read_http_request(socket: &mut TcpStream) -> Result<Value> {
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 8192];
        let read = socket.read(&mut chunk).await.context("read request")?;
        anyhow::ensure!(read != 0, "socket closed before request headers completed");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers =
        String::from_utf8(data[..header_end].to_vec()).context("request headers UTF-8")?;
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);
    while data.len() < header_end + length {
        let mut chunk = [0; 8192];
        let read = socket.read(&mut chunk).await.context("read request body")?;
        anyhow::ensure!(read != 0, "socket closed mid-request-body");
        data.extend_from_slice(&chunk[..read]);
    }
    let body = std::str::from_utf8(&data[header_end..header_end + length])?;
    serde_json::from_str(body).context("request body JSON")
}

/// Wait until the peer closes (EOF or connection teardown) within the bound.
async fn expect_socket_release(socket: &mut TcpStream) -> Result<()> {
    let mut probe = [0; 1];
    match tokio::time::timeout(Duration::from_secs(3), socket.read(&mut probe)).await {
        Ok(Ok(0)) => Ok(()),
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ) =>
        {
            Ok(())
        }
        Ok(Ok(read)) => Err(anyhow::anyhow!(
            "interrupted socket received {read} unexpected bytes"
        )),
        Ok(Err(error)) => Err(anyhow::anyhow!("observe interrupted socket: {error}")),
        Err(error) => Err(anyhow::anyhow!(
            "interrupted request socket did not close within three seconds: {error}"
        )),
    }
}

#[derive(Clone, Copy)]
enum Stall {
    /// Send no response bytes at all; the interrupt must cancel while
    /// reqwest is still waiting for response headers.
    BeforeHeaders,
    /// Send the SSE head plus real deltas, then stall; the interrupt must
    /// cancel mid-stream after partial content was consumed.
    MidDelta,
}

/// Drive one interrupt round-trip on `wire`:
/// request 1 stalls per `stall` and must be released by `Op::Interrupt`;
/// request 2 is answered with a complete assistant turn. `request_ready`
/// fires once the stalled request has been fully received.
async fn serve_interrupt_round(
    listener: TcpListener,
    wire: WireApi,
    stall: Stall,
    request_ready: oneshot::Sender<()>,
) -> Result<Vec<Value>> {
    let (mut first_socket, _) = listener.accept().await.context("accept stalled request")?;
    let first_request = read_http_request(&mut first_socket).await?;
    request_ready
        .send(())
        .map_err(|_| anyhow::anyhow!("request-ready receiver closed"))?;
    match stall {
        Stall::BeforeHeaders => {}
        Stall::MidDelta => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{}",
                partial_sse_prefix(wire)
            );
            first_socket
                .write_all(head.as_bytes())
                .await
                .context("write partial SSE")?;
        }
    }
    expect_socket_release(&mut first_socket).await?;
    drop(first_socket);

    let (mut second_socket, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
        .await
        .context("follow-up request did not arrive")?
        .context("accept follow-up request")?;
    let second_request = read_http_request(&mut second_socket).await?;
    let follow_up = complete_sse(wire, "follow-up completed");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{follow_up}",
        follow_up.len()
    );
    second_socket
        .write_all(response.as_bytes())
        .await
        .context("write complete follow-up SSE")?;
    drop(second_socket);
    anyhow::ensure!(
        tokio::time::timeout(Duration::from_millis(750), listener.accept())
            .await
            .is_err(),
        "interrupt or follow-up produced an extra model connection"
    );
    Ok(vec![first_request, second_request])
}

async fn run_interrupt_case(wire: WireApi, stall: Stall) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (request_ready_tx, request_ready_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        serve_interrupt_round(listener, wire, stall, request_ready_tx).await
    });

    let server = MockServer::start().await;
    let provider = interrupt_provider(&format!("http://{address}"), wire);
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    let codex = std::sync::Arc::clone(&test.codex);
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "stall".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    // Fixture-robustness deadline only (session startup competes with the
    // other parallel suite tests); the contracts below stay tight.
    tokio::time::timeout(Duration::from_secs(30), request_ready_rx)
        .await
        .context("stalled request did not reach the server")?
        .context("stalled-request server stopped before readiness")?;
    codex
        .submit(codex_protocol::protocol::Op::Interrupt)
        .await?;

    let mut saw_turn_complete = false;
    let aborted = core_test_support::wait_for_event(&codex, |event| {
        if matches!(event, EventMsg::TurnComplete(_)) {
            saw_turn_complete = true;
        }
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;
    anyhow::ensure!(
        matches!(aborted, EventMsg::TurnAborted(_)),
        "expected TurnAborted for the interrupted turn, got {aborted:?}"
    );
    anyhow::ensure!(
        !saw_turn_complete,
        "the interrupted turn must never fake a TurnComplete"
    );

    // Submit immediately after TurnAborted: the same Core client must stay
    // usable without a manual reset or delayed retry.
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "follow up immediately".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut assistant_messages = Vec::new();
    let completed = core_test_support::wait_for_event(&codex, |event| {
        if let EventMsg::AgentMessage(message) = event {
            assistant_messages.push(message.message.clone());
        }
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let EventMsg::TurnComplete(completed) = completed else {
        anyhow::bail!("expected follow-up TurnComplete");
    };
    assert_eq!(completed.error, None, "immediate follow-up must succeed");
    assert_eq!(
        completed.last_agent_message.as_deref(),
        Some("follow-up completed")
    );
    assert_eq!(assistant_messages, vec!["follow-up completed"]);

    let requests = server_task.await.context("join raw gateway")??;
    assert_eq!(
        requests.len(),
        2,
        "exactly the interrupted request and one follow-up; no extra attempts"
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_interrupt_before_headers_closes_socket_and_allows_follow_up() -> Result<()> {
    skip_if_no_network!(Ok(()));
    run_interrupt_case(WireApi::Chat, Stall::BeforeHeaders).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_interrupt_mid_delta_aborts_and_allows_follow_up() -> Result<()> {
    skip_if_no_network!(Ok(()));
    run_interrupt_case(WireApi::Chat, Stall::MidDelta).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_interrupt_before_headers_closes_socket_and_allows_follow_up() -> Result<()> {
    skip_if_no_network!(Ok(()));
    run_interrupt_case(WireApi::Responses, Stall::BeforeHeaders).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_interrupt_mid_delta_aborts_and_allows_follow_up() -> Result<()> {
    skip_if_no_network!(Ok(()));
    run_interrupt_case(WireApi::Responses, Stall::MidDelta).await
}
