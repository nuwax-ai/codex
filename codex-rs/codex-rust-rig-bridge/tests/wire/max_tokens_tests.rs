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

async fn run_with_budget(protocol: RigProtocol, budget: Option<u64>) -> serde_json::Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = provider(listener.local_addr().unwrap());
    provider.max_output_tokens = budget;
    let payload = match protocol {
        RigProtocol::Chat => support::CHAT_SSE.to_string(),
        RigProtocol::Anthropic => support::ANTHROPIC_SSE.to_string(),
        RigProtocol::Responses => support::RESPONSES_SSE.to_string(),
    };
    let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
    let request = support::request(vec![support::user()]);
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
    let wire = run_with_budget(RigProtocol::Anthropic, Some(2_048)).await;
    assert_eq!(wire["body"]["max_tokens"], json!(2_048));
}

#[tokio::test]
async fn provider_output_budget_reaches_the_chat_wire() {
    let wire = run_with_budget(RigProtocol::Chat, Some(1_024)).await;
    assert_eq!(wire["body"]["max_tokens"], json!(1_024));
}

#[tokio::test]
async fn anthropic_keeps_the_bridge_default_without_a_budget() {
    let wire = run_with_budget(RigProtocol::Anthropic, None).await;
    assert_eq!(
        wire["body"]["max_tokens"],
        json!(codex_rust_rig_bridge::DEFAULT_ANTHROPIC_MAX_TOKENS)
    );
}
