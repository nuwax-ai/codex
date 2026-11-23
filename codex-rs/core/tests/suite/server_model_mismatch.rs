//! A reply that names another model is a reroute only on OpenAI's own backend.
//!
//! Fork (nuwax-codex): the upstream check tells a user their account was
//! flagged for high-risk cyber activity whenever the reply's `model` differs
//! from the requested one. Third-party gateways name a snapshot, an alias or
//! their upstream channel's model as a matter of course, so for them that
//! message is a false alarm. These tests drive the FULL core sampling loop
//! over the rig bridge's Chat wire and read what the user is told.
//!
//! Feature gate: run with
//! `cargo nextest run -p codex-core --features rust-rig -E 'test(server_model_mismatch)'`
//! and verify the selected count is non-zero.

#![cfg(feature = "rust-rig")]

use anyhow::Context;
use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ModelRerouteEvent;
use codex_protocol::protocol::ModelRerouteReason;
use codex_protocol::user_input::UserInput;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use wiremock::MockServer;

const REQUESTED: &str = "mismatch-test-model";
/// A dated snapshot name, as a gateway reports the model it actually served.
const REPORTED: &str = "mismatch-test-model-2099-01-01";
const CYBER_WARNING: &str = "flagged for potentially high-risk cyber activity";

#[derive(Debug, Default)]
struct WhatTheUserHeard {
    reroutes: Vec<ModelRerouteEvent>,
    warnings: Vec<String>,
    final_message: Option<String>,
    turn_error: Option<String>,
}

fn provider(base_url: &str, provider_id: &str) -> ModelProviderInfo {
    ModelProviderInfo {
        capabilities: None,
        api_key: None,
        models: None,
        name: "mismatch-test".into(),
        base_url: Some(format!("{base_url}/v1")),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        experimental_bridge: None,
        provider_id: Some(provider_id.into()),
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api: WireApi::Chat,
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
        project_id: None,
        location: None,
        credential_file: None,
        include_internal_metadata: false,
        max_output_tokens: Some(64),
        hosted_results_replay: None,
        anthropic_prompt_caching: None,
        responses_trailing_tool_result_user_turn: None,
        reasoning_effort_levels: None,
    }
}

/// One complete Chat reply that names `model` in every chunk.
fn chat_reply(model: &str) -> String {
    let chunk = |delta: Value, finish: Option<&str>, usage: Value| {
        let mut choice = json!({"index": 0, "delta": delta});
        choice["finish_reason"] = finish.map(Value::from).unwrap_or(Value::Null);
        json!({
            "id": "chatcmpl-mismatch", "object": "chat.completion.chunk", "created": 1,
            "model": model, "choices": [choice], "usage": usage,
        })
    };
    let unused = json!(null);
    let frames = [
        chunk(
            json!({"role": "assistant", "content": "OK"}),
            None,
            unused.clone(),
        ),
        chunk(json!({}), Some("stop"), unused.clone()),
        json!({
            "id": "chatcmpl-mismatch", "object": "chat.completion.chunk", "created": 1,
            "model": model, "choices": [],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4},
        }),
    ];
    let mut body = String::new();
    for frame in frames {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

async fn instance(provider_id: &str) -> Result<(TestCodex, MockServer)> {
    let server = MockServer::start().await;
    let provider = provider(&server.uri(), provider_id);
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(chat_reply(REPORTED), "text/event-stream"),
        )
        .mount(&server)
        .await;
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model = Some(REQUESTED.into());
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    Ok((test, server))
}

async fn run_turn(test: &TestCodex) -> Result<WhatTheUserHeard> {
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "say OK".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut heard = WhatTheUserHeard::default();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(60), test.codex.next_event())
            .await
            .context("timed out waiting for a turn event")?
            .context("event stream ended before TurnComplete")?;
        match event.msg {
            EventMsg::AgentMessage(message) => heard.final_message = Some(message.message),
            EventMsg::ModelReroute(event) => heard.reroutes.push(event),
            EventMsg::Warning(event) => heard.warnings.push(event.message),
            EventMsg::TurnComplete(complete) => {
                heard.turn_error = complete.error.map(|error| error.message);
                return Ok(heard);
            }
            _ => {}
        }
    }
}

/// The request still carries the model the operator chose; only the reply's
/// name differs, which is the gateway's to give.
async fn assert_requested_model(server: &MockServer) {
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1, "one request per turn");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("request body is JSON");
    assert_eq!(body["model"], REQUESTED);
    assert_ne!(REPORTED, REQUESTED);
}

#[tokio::test]
async fn a_third_party_gateway_naming_another_model_is_not_a_reroute() -> Result<()> {
    let (test, server) = instance("corp-gateway").await?;
    let heard = run_turn(&test).await?;
    assert_requested_model(&server).await;
    assert_eq!(heard.turn_error, None);
    assert_eq!(heard.final_message.as_deref(), Some("OK"));
    assert_eq!(
        heard.reroutes,
        Vec::<ModelRerouteEvent>::new(),
        "a gateway's snapshot or alias name is not a reroute"
    );
    assert!(
        !heard
            .warnings
            .iter()
            .any(|message| message.contains(CYBER_WARNING)),
        "nobody's account was flagged: {:?}",
        heard.warnings
    );
    Ok(())
}

/// The reason the check exists: OpenAI's backend reroutes a flagged account
/// and the reply's model is the only trace of it.
#[tokio::test]
async fn openai_naming_another_model_is_still_reported_as_a_reroute() -> Result<()> {
    let (test, _server) = instance(OPENAI_PROVIDER_ID).await?;
    let heard = run_turn(&test).await?;
    assert_eq!(heard.turn_error, None);
    assert_eq!(heard.final_message.as_deref(), Some("OK"));
    assert_eq!(
        heard.reroutes,
        vec![ModelRerouteEvent {
            from_model: REQUESTED.into(),
            to_model: REPORTED.into(),
            reason: ModelRerouteReason::HighRiskCyberActivity,
        }]
    );
    assert_eq!(heard.warnings.len(), 1);
    assert!(heard.warnings[0].contains(CYBER_WARNING));
    Ok(())
}
