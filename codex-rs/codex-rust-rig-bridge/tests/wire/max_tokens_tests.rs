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
