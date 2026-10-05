//! Wire tests for the provider-level output budget (`max_output_tokens`):
//! the configured value must reach the wire `max_tokens` on both chat-family
//! protocols, and Anthropic must still carry the bridge default when unset.

use super::error_tests::provider;
use super::support;
use codex_api::SharedAuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

async fn run_with_budget(
    protocol: RigProtocol,
    budget: Option<u64>,
    request_budget: Option<u64>,
) -> serde_json::Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = provider(listener.local_addr().unwrap());
    provider.max_output_tokens = budget;
    let payload = match protocol {
        RigProtocol::Chat => support::CHAT_SSE.to_string(),
        RigProtocol::Anthropic => support::ANTHROPIC_SSE.to_string(),
        RigProtocol::Responses => support::RESPONSES_SSE.to_string(),
    };
    let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
    let mut request = support::request(vec![support::user()]);
    request.max_output_tokens = request_budget;
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        protocol,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    server.await.unwrap()
}

#[tokio::test]
async fn provider_output_budget_reaches_the_anthropic_wire() {
    let wire = run_with_budget(RigProtocol::Anthropic, Some(2_048), None).await;
    assert_eq!(wire["body"]["max_tokens"], json!(2_048));
}

#[tokio::test]
async fn provider_output_budget_reaches_the_chat_wire() {
    let wire = run_with_budget(RigProtocol::Chat, Some(1_024), None).await;
    assert_eq!(wire["body"]["max_tokens"], json!(1_024));
}

#[tokio::test]
async fn anthropic_keeps_the_bridge_default_without_a_budget() {
    let wire = run_with_budget(RigProtocol::Anthropic, None, None).await;
    assert_eq!(
        wire["body"]["max_tokens"],
        json!(codex_rust_rig_bridge::DEFAULT_ANTHROPIC_MAX_TOKENS)
    );
}

#[tokio::test]
async fn provider_output_budget_reaches_responses_and_unset_stays_absent() {
    for budget in [None, Some(2048)] {
        let wire = run_with_budget(RigProtocol::Responses, budget, None).await;
        assert_eq!(
            wire["body"].get("max_output_tokens"),
            budget.map(|cap| json!(cap)).as_ref()
        );
    }
}

#[tokio::test]
async fn request_output_budget_overrides_provider_on_every_protocol() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let wire = run_with_budget(protocol, Some(4096), Some(2048)).await;
        let field = if protocol == RigProtocol::Responses {
            "max_output_tokens"
        } else {
            "max_tokens"
        };
        assert_eq!(wire["body"][field], json!(2048));
    }
}

#[tokio::test]
async fn chat_length_returns_terminal_output_cap_error_with_partial_text() {
    // A cap terminal keeps emitted deltas but does not synthesize a successful
    // Completed/usage event. Core resampling is verified at a different layer.
    let payload =
        support::CHAT_SSE.replace("\"finish_reason\":\"stop\"", "\"finish_reason\":\"length\"");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        // Serve up to two requests; the cap-exhausted turn must need exactly one.
        let mut bodies = Vec::new();
        for _ in 0..2 {
            let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_secs(2), listener.accept()).await
            else {
                break;
            };
            let request = support::read_request(&mut socket).await;
            bodies.push(request["body"].clone());
            use tokio::io::AsyncWriteExt;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
        bodies
    });
    let provider = provider(address);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut request = support::request(vec![support::user()]);
    request.max_output_tokens = Some(64);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Chat,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let mut texts = Vec::new();
    let mut terminal = None;
    let mut completed = 0;
    let mut done = 0;
    let mut errors = 0;
    while let Some(event) = stream.next().await {
        match event {
            Ok(codex_api::ResponseEvent::OutputTextDelta(text)) => texts.push(text),
            Ok(codex_api::ResponseEvent::Completed { .. }) => completed += 1,
            Ok(codex_api::ResponseEvent::OutputItemDone(_)) => done += 1,
            Ok(_) => {}
            Err(error) => {
                errors += 1;
                terminal = Some(error);
            }
        }
    }
    let bodies = server.await.unwrap();
    // Output exhaustion is a terminal budget condition: one request, the
    // already-streamed partial text stays visible, and the terminal error is
    // the non-retryable "increase the cap" InvalidRequest (core's
    // retry_delay is None, so the same budget is never resampled).
    let error = terminal.expect("terminal budget error");
    assert!(matches!(
        &error,
        codex_api::ApiError::InvalidRequest { message }
            if message.contains("Output token limit reached")
    ));
    assert_eq!(
        (
            bodies.len(),
            texts.concat(),
            completed,
            done,
            errors,
            bodies[0]["max_tokens"].clone()
        ),
        (1, "ok".to_string(), 0, 0, 1, json!(64))
    );
}

#[tokio::test]
async fn anthropic_max_tokens_returns_terminal_output_cap_error_with_partial_text() {
    // Same partial-text and non-successful terminal policy as Chat.
    let payload = support::ANTHROPIC_SSE.replace(
        "\"stop_reason\":\"end_turn\"",
        "\"stop_reason\":\"max_tokens\"",
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for _ in 0..2 {
            let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_secs(2), listener.accept()).await
            else {
                break;
            };
            let request = support::read_request(&mut socket).await;
            bodies.push(request["body"].clone());
            use tokio::io::AsyncWriteExt;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
        bodies
    });
    let provider = provider(address);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut request = support::request(vec![support::user()]);
    request.max_output_tokens = Some(64);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let mut texts = Vec::new();
    let mut terminal = None;
    let mut completed = 0;
    let mut done = 0;
    let mut errors = 0;
    while let Some(event) = stream.next().await {
        match event {
            Ok(codex_api::ResponseEvent::OutputTextDelta(text)) => texts.push(text),
            Ok(codex_api::ResponseEvent::Completed { .. }) => completed += 1,
            Ok(codex_api::ResponseEvent::OutputItemDone(_)) => done += 1,
            Ok(_) => {}
            Err(error) => {
                errors += 1;
                terminal = Some(error);
            }
        }
    }
    let bodies = server.await.unwrap();
    let error = terminal.expect("terminal budget error");
    assert!(matches!(
        &error,
        codex_api::ApiError::InvalidRequest { message }
            if message.contains("Output token limit reached")
    ));
    assert_eq!(
        (
            bodies.len(),
            texts.concat(),
            completed,
            done,
            errors,
            bodies[0]["max_tokens"].clone()
        ),
        (1, "ok".to_string(), 0, 0, 1, json!(64))
    );
}
