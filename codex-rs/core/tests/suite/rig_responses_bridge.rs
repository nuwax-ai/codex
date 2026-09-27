//! End-to-end coverage for the fork's default third-party routing: a
//! `wire_api = "responses"` provider with no `experimental_bridge` must be
//! served by the rig bridge on the SAME Responses wire (`POST /v1/responses`),
//! with model-generated outputs persisting provenance for phase-2 projection.
//!
//! Gated on `rust-rig`: without bridge features the provider is rejected
//! before dispatch (see `core/src/client.rs`).

#![cfg(feature = "rust-rig")]

use codex_core::TurnInputRequest;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use wiremock::MockServer;

fn bridged_responses_provider(server: &MockServer) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "rig-responses".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        // No experimental_bridge: the fork default routes through rig, which
        // now speaks the SAME Responses wire instead of converting to Chat.
        experimental_bridge: None,
        provider_id: None,
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
    }
}

type Conversation = std::sync::Arc<codex_core::CodexThread>;

async fn submit_user_turn(codex: &Conversation, text: &str) {
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }]))
        .await
        .expect("submit user turn");
    wait_for_event(codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
}

/// The bridged provider hits `/v1/responses` (not chat/completions), completes
/// a turn, persists outputs, and every model-generated envelope records the
/// producing request's provenance — captured at persist time, never backfilled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_bridge_same_wire_round_trip_and_provenance() {
    skip_if_no_network!();

    let server = MockServer::start().await;
    let turn_one = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_assistant_message("msg-1", "first answer"),
        responses::ev_completed("resp-1"),
    ]);
    let turn_two = responses::sse(vec![
        responses::ev_response_created("resp-2"),
        responses::ev_assistant_message("msg-2", "second answer"),
        responses::ev_completed("resp-2"),
    ]);
    let response_mock = responses::mount_sse_sequence(&server, vec![turn_one, turn_two]).await;

    let provider = bridged_responses_provider(&server);
    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await
        .expect("build test codex");
    let codex: Conversation = codex;

    submit_user_turn(&codex, "hello").await;

    let first = response_mock.single_request();
    assert_eq!(
        first.path(),
        "/v1/responses",
        "must speak the Responses wire"
    );

    // Second turn in the same session: history replay must carry the first
    // turn's assistant message verbatim with pairing intact.
    submit_user_turn(&codex, "again").await;
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "expected exactly two model requests");
    let second_input = requests[1].input();
    assert!(
        second_input.iter().any(|item| {
            item.get("type").and_then(|value| value.as_str()) == Some("message")
                && item.get("role").and_then(|value| value.as_str()) == Some("assistant")
                && format!("{item}").contains("first answer")
        }),
        "second request must replay the first turn's assistant message: {second_input:?}"
    );

    // Persisted provenance on model-generated envelopes.
    let rollout_path = codex.rollout_path().expect("rollout path");
    let contents = std::fs::read_to_string(&rollout_path).expect("read rollout");
    let mut assistant_provenance = Vec::new();
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(line) = codex_rollout::parse_rollout_line(line) else {
            continue;
        };
        let ResponseItemEnvelope { item, metadata } = match line.item {
            RolloutItem::ResponseItem(envelope) => envelope,
            _ => continue,
        };
        if matches!(
            &item,
            codex_protocol::models::ResponseItem::Message { role, .. } if role == "assistant"
        ) {
            assistant_provenance.push(
                metadata
                    .as_ref()
                    .and_then(|metadata| metadata.model_output_provenance.clone()),
            );
        }
    }
    assert_eq!(
        assistant_provenance.len(),
        2,
        "both assistant outputs must be persisted"
    );
    for provenance in assistant_provenance {
        let provenance = provenance.expect("assistant outputs record provenance");
        assert_eq!(provenance.wire_protocol, "responses");
        assert_eq!(provenance.bridge.as_deref(), Some("rig"));
        assert_eq!(provenance.provider.as_deref(), Some("rig-responses"));
        assert!(provenance.model.is_some(), "model slug recorded");
    }
}
