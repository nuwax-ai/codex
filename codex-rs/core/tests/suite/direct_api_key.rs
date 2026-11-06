//! Direct `api_key` credential mode (my-docs/direct-provider-api-key-2026-11-05):
//! a provider whose key lives in config must authenticate without any
//! environment variable, on all three wires, and two providers must not
//! cross keys. These run real loopback HTTP and assert the actual
//! `Authorization` header that reached the server.

#![cfg(feature = "rust-rig")]

use anyhow::Result;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_utils_redacted_string::RedactedString;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::MockServer;
use wiremock::Request;

fn direct_key_provider(server: &MockServer, wire_api: WireApi, key: &str) -> ModelProviderInfo {
    let protocol_name = match wire_api {
        WireApi::Chat => "chat",
        WireApi::Responses => "responses",
        WireApi::Anthropic => "anthropic",
    };
    ModelProviderInfo {
        capabilities: None,
        api_key: Some(RedactedString::from(key.to_string())),
        models: None,
        name: format!("direct-key-{protocol_name}"),
        base_url: Some(format!("{}/v1", server.uri())),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        experimental_bridge: None,
        provider_id: Some(format!("direct-key-{protocol_name}")),
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api,
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
        max_output_tokens: None,
        hosted_results_replay: None,
        anthropic_prompt_caching: None,
        reasoning_effort_levels: None,
    }
}

fn chat_completion_sse() -> String {
    let chunks = [
        json!({"id":"chatcmpl-direct","object":"chat.completion.chunk","created":1,
               "model":"direct-key-test","choices":[{"index":0,
               "delta":{"content":"ok"},"finish_reason":null}]}),
        json!({"id":"chatcmpl-direct","object":"chat.completion.chunk","created":1,
               "model":"direct-key-test","choices":[{"index":0,
               "delta":{},"finish_reason":"stop"}]}),
    ];
    chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect()
}

fn anthropic_completion_sse() -> String {
    [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_direct\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n".to_string(),
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_string(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n".to_string(),
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".to_string(),
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":6}}\n\n".to_string(),
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string(),
    ]
    .concat()
}

fn responses_completion_sse() -> String {
    responses::sse(vec![
        responses::ev_response_created("resp_direct"),
        responses::ev_assistant_message("msg_direct", "ok"),
        responses::ev_completed("resp_direct"),
    ])
}

/// Direct-key providers enable API-key model discovery; the first turn can
/// sit silent for longer than the harness's 10s event floor while discovery
/// settles, so turns here use a patient waiter (the assertions are about the
/// wire headers, not latency).
async fn submit_turn_patient(
    test: &core_test_support::test_codex::TestCodex,
    prompt: &str,
) -> Result<()> {
    use codex_core::TurnInputRequest;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::user_input::UserInput;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: prompt.into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    loop {
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(45), test.codex.next_event())
                .await
                .expect("timed out waiting for a turn event")?;
        if matches!(event.msg, EventMsg::TurnComplete(_)) {
            return Ok(());
        }
    }
}

/// Direct-key providers enable API-key model discovery; serve an empty
/// catalog instantly so the first turn is not delayed by 404 backoff.
async fn mount_models_discovery(server: &MockServer) {
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1/models"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"object":"list","data":[]})),
        )
        .mount(server)
        .await;
}

async fn mount_sse(server: &MockServer, path: &str, body: String) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(path))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(body, "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// Runs one turn against a direct-key provider and returns the loopback
/// request that reached the server — no environment variable involved (the
/// test process never defines one for these providers).
async fn run_direct_key_turn(
    wire_api: WireApi,
    path: &str,
    body: String,
    key: &str,
) -> Result<Request> {
    let server = MockServer::start().await;
    mount_sse(&server, path, body).await;
    let provider = direct_key_provider(&server, wire_api, key);
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build(&server)
        .await?;
    submit_turn_patient(&test, "hello").await?;
    let requests = server.received_requests().await.expect("wiremock requests");
    Ok(requests
        .into_iter()
        .find(|request| request.url.path() == path)
        .expect("loopback request reached the provider"))
}

fn assert_bearer(request: &Request, key: &str) {
    assert_eq!(
        request
            .headers
            .get("authorization")
            .expect("authorization header present"),
        format!("Bearer {key}").as_str()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_api_key_authenticates_responses_wire() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let request = run_direct_key_turn(
        WireApi::Responses,
        "/v1/responses",
        responses_completion_sse(),
        "sk-direct-responses-key",
    )
    .await?;
    assert_bearer(&request, "sk-direct-responses-key");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_api_key_authenticates_chat_wire() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let request = run_direct_key_turn(
        WireApi::Chat,
        "/v1/chat/completions",
        chat_completion_sse(),
        "sk-direct-chat-key",
    )
    .await?;
    assert_bearer(&request, "sk-direct-chat-key");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_api_key_authenticates_anthropic_wire() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let request = run_direct_key_turn(
        WireApi::Anthropic,
        "/v1/messages",
        anthropic_completion_sse(),
        "sk-direct-anthropic-key",
    )
    .await?;
    // The Anthropic wire spells the bearer credential as x-api-key (the
    // bridge translates; see client.rs's header filter).
    assert_eq!(
        request
            .headers
            .get("x-api-key")
            .expect("x-api-key header present"),
        "sk-direct-anthropic-key"
    );
    Ok(())
}

/// Two providers with different keys never cross credentials: each loopback
/// server sees only its own provider's bearer token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_api_key_providers_do_not_cross_keys() -> Result<()> {
    skip_if_no_network!(Ok(()));
    for (provider_id, key) in [
        ("direct-key-a", "sk-key-a-only"),
        ("direct-key-b", "sk-key-b-only"),
    ] {
        let server = MockServer::start().await;
        mount_models_discovery(&server).await;
        mount_sse(&server, "/v1/chat/completions", chat_completion_sse()).await;
        let mut provider = direct_key_provider(&server, WireApi::Chat, key);
        provider.provider_id = Some(provider_id.to_string());
        let test = test_codex()
            .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
                /*auth*/ None,
            ))
            .with_config(move |config| {
                config.model_provider = provider;
            })
            .build(&server)
            .await?;
        submit_turn_patient(&test, "hello").await?;
        let requests = server.received_requests().await.expect("wiremock requests");
        let request = requests
            .into_iter()
            .find(|request| request.url.path() == "/v1/chat/completions")
            .expect("loopback request reached the provider");
        assert_bearer(&request, key);
    }
    Ok(())
}
