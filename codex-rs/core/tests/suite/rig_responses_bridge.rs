//! End-to-end coverage for the fork's default third-party routing: a
//! `wire_api = "responses"` provider with no `experimental_bridge` must be
//! served by the rig bridge on the SAME Responses wire (`POST /v1/responses`),
//! with model-generated outputs persisting provenance for phase-2 projection.
//!
//! Gated on `rust-rig`: without bridge features the provider is rejected
//! before dispatch (see `core/src/client.rs`).

#![cfg(feature = "rust-rig")]

use anyhow::Context;
use anyhow::Result;
use codex_history::ModelOutputProvenance;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
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
        provider_id: Some("rig-responses".into()),
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
        include_internal_metadata: false,
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

fn persisted_response_items(contents: &str) -> Result<Vec<ResponseItemEnvelope>> {
    let mut items = Vec::new();
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        if let RolloutItem::ResponseItem(item) = codex_rollout::parse_rollout_line(line)?.item {
            items.push(item);
        }
    }
    Ok(items)
}

/// Exercise both request shapes through core dispatch, including the header
/// that selects the additional_tools input format on Responses Lite servers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_bridge_preserves_lite_request_contract() -> Result<()> {
    skip_if_no_network!(Ok(()));

    for use_responses_lite in [false, true] {
        let server = MockServer::start().await;
        let response_mock = responses::mount_sse_once(
            &server,
            responses::sse(vec![
                responses::ev_response_created("resp_lite"),
                responses::ev_assistant_message("msg_lite", "done"),
                responses::ev_completed("resp_lite"),
            ]),
        )
        .await;
        let mut provider = bridged_responses_provider(&server);
        provider.max_output_tokens = Some(4096);
        let test = test_codex()
            .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
                /*auth*/ None,
            ))
            .with_model_info_override("rig-lite-test-model", move |model| {
                model.use_responses_lite = use_responses_lite;
            })
            .with_config(move |config| {
                config.model_provider = provider;
                config.base_instructions = Some("bridge instructions".into());
            })
            .build_with_auto_env(&server)
            .await?;
        test.submit_text_turn("hello").await?;

        let request = response_mock.single_request();
        assert_eq!(request.path(), "/v1/responses");
        assert_eq!(
            request.header("x-openai-internal-codex-responses-lite"),
            use_responses_lite.then(|| "true".to_string()),
        );
        let body = request.body_json();
        assert_eq!(body["max_output_tokens"], json!(4096));
        let additional_tools = request.inputs_of_type("additional_tools");
        if use_responses_lite {
            assert!(body.get("tools").is_none());
            assert!(body.get("instructions").is_none());
            assert_eq!(additional_tools.len(), 1);
            assert!(
                !additional_tools[0]["tools"]
                    .as_array()
                    .context("tools")?
                    .is_empty()
            );
        } else {
            assert!(body["tools"].is_array());
            assert_eq!(body["instructions"], "bridge instructions");
            assert!(additional_tools.is_empty());
        }
        test.codex.shutdown_and_wait().await?;
    }
    Ok(())
}

/// Save a real tool round trip, close the session, then resume with a new
/// provider/model. Old provenance (including its absence in legacy records)
/// and tool pairing must survive without entering the next model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_bridge_resumes_history_without_backfilling_provenance() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let mut reasoning =
        responses::ev_reasoning_item("rs_original", &["saved summary"], &["saved thought"]);
    reasoning["item"]["encrypted_content"] = "opaque-original".into();
    let assistant = responses::ev_assistant_message("msg_original", "first answer");
    // Versioned same-source envelope (R2): persisted verbatim through the
    // rollout and cleared from the Responses request copy below.
    let hosted_item = json!({
        "type":"web_search_call", "id":"search_saved", "status":"completed",
        "action":{"type":"search", "query":"saved query"},
        "wire_blocks":{
            "version":1,
            "source":"Anthropic:0000000000000000000000000000000000000000000000000000000000000000",
            "blocks":[
                {"type":"server_tool_use", "id":"srv_saved", "name":"web_search", "input":{"query":"saved query"}},
                {"type":"web_search_tool_result", "tool_use_id":"srv_saved", "content":[{"type":"web_search_result", "encrypted_content":"vendor-secret"}]}
            ],
            "cited_text":[
                {"type":"text", "text":"cited answer",
                 "citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","search_result_index":0,"start_block_index":1,"end_block_index":2}]}
            ]
        }
    });
    let plan_args = json!({"plan": [{"step": "save history", "status": "completed"}]}).to_string();
    let initial_mock = responses::mount_response_sequence(
        &server,
        vec![
            responses::sse_response(responses::sse(vec![
                responses::ev_response_created("resp_tool"),
                reasoning.clone(),
                responses::ev_function_call("call_plan", "update_plan", &plan_args),
                responses::ev_completed("resp_tool"),
            ]))
            .insert_header("x-codex-turn-state", "ts_rig_tool"),
            responses::sse_response(responses::sse(vec![
                responses::ev_response_created("resp_original"),
                assistant.clone(),
                json!({"type":"response.output_item.done", "item":hosted_item}),
                responses::ev_completed("resp_original"),
            ])),
        ],
    )
    .await;
    let provider = bridged_responses_provider(&server);
    let initial = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_model_info_override("rig-original-model", |model| {
            model.use_responses_lite = false;
            model.tool_mode = None;
        })
        .with_config(move |config| {
            config.model_provider = provider;
            config.update_plan_enabled = true;
        })
        .build_with_auto_env(&server)
        .await?;
    initial.submit_text_turn("update the plan").await?;
    initial.codex.shutdown_and_wait().await?;

    let requests = initial_mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]
            .input()
            .iter()
            .find(|item| item["id"] == "rs_original")
            .context("same-source reasoning continuation")?["encrypted_content"],
        "opaque-original"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.path() == "/v1/responses")
    );
    assert_eq!(requests[0].header("x-codex-turn-state"), None);
    assert_eq!(
        requests[1].header("x-codex-turn-state"),
        Some("ts_rig_tool".to_string()),
    );
    assert_eq!(
        requests[1]
            .function_call_output_text("call_plan")
            .as_deref(),
        Some("Plan updated"),
    );
    let tool_pair: Vec<_> = requests[1]
        .input()
        .into_iter()
        .filter(|item| item["call_id"] == "call_plan")
        .collect();
    assert_eq!(tool_pair.len(), 2);

    let rollout_path = initial.codex.rollout_path().context("rollout path")?;
    let original_contents = std::fs::read_to_string(&rollout_path)?;
    let original_items = persisted_response_items(&original_contents)?;
    let saved_search = original_items
        .iter()
        .find(|item| item.id().is_some_and(|id| id.as_str() == "search_saved"))
        .context("persisted hosted search")?;
    let mut saved_search_payload = serde_json::to_value(&saved_search.item)?;
    // Core stamps its own turn metadata while persisting output. Compare the
    // provider payload separately from that dynamic, runtime-owned field.
    saved_search_payload
        .as_object_mut()
        .context("saved search object")?
        .remove("internal_chat_message_metadata_passthrough");
    assert_eq!(saved_search_payload, hosted_item);
    let original_provenance = ModelOutputProvenance {
        wire_protocol: "responses".into(),
        bridge: Some("rig".into()),
        provider: Some("rig-responses".into()),
        model: Some("rig-original-model".into()),
        endpoint_identity: codex_api::model_endpoint_identity(
            &bridged_responses_provider(&server).to_api_provider(None)?,
        ),
        auth_domain: Some("anonymous".into()),
        auth_domain_kind: Some("anonymous".into()),
    };
    for id in ["rs_original", "msg_original", "search_saved"] {
        let item = original_items
            .iter()
            .find(|item| item.id().is_some_and(|item_id| item_id.as_str() == id))
            .context("original model output")?;
        assert_eq!(
            item.metadata
                .as_ref()
                .and_then(|metadata| metadata.model_output_provenance.as_ref()),
            Some(&original_provenance),
        );
    }

    // Simulate one legacy record after the producing runtime is closed. The
    // remaining output retains its original provenance for the same resume.
    let mut legacy_lines = Vec::new();
    let mut legacy_records = 0;
    for line in original_contents.lines() {
        let mut line: Value = serde_json::from_str(line)?;
        if line["type"] == "response_item" && line["payload"]["id"] == "rs_original" {
            line["metadata"]
                .as_object_mut()
                .context("reasoning metadata")?
                .remove("model_output_provenance");
            legacy_records += 1;
        }
        legacy_lines.push(serde_json::to_string(&line)?);
    }
    assert_eq!(legacy_records, 1);
    let saved_contents = format!("{}\n", legacy_lines.join("\n"));
    std::fs::write(&rollout_path, &saved_contents)?;
    let saved_items = persisted_response_items(&saved_contents)?;

    let resumed_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp_resumed"),
            responses::ev_assistant_message("msg_resumed", "second answer"),
            responses::ev_completed("resp_resumed"),
        ]),
    )
    .await;
    let mut provider = bridged_responses_provider(&server);
    provider.name = "rig-responses-resumed".into();
    provider.provider_id = Some("rig-responses-resumed".into());
    let original_cwd = initial.config.cwd.clone();
    let mut resume_builder = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_model_info_override("rig-resumed-model", |model| {
            model.use_responses_lite = false;
            model.tool_mode = None;
        })
        .with_config(move |config| {
            config.model_provider = provider;
            config.update_plan_enabled = true;
            config.cwd = original_cwd;
        });
    if let Some(exec_server_url) = initial.executor_environment().exec_server_url() {
        resume_builder = resume_builder.with_exec_server_url(exec_server_url);
    }
    let resumed = resume_builder
        .resume(&server, initial.home.clone(), rollout_path.clone())
        .await?;
    resumed.submit_text_turn("continue").await?;
    resumed.codex.shutdown_and_wait().await?;

    let request = resumed_mock.single_request();
    assert_eq!(request.path(), "/v1/responses");
    assert_eq!(request.header("x-codex-turn-state"), None);
    assert_eq!(request.body_json()["model"], "rig-resumed-model");
    let input = request.input();
    assert_eq!(
        input
            .iter()
            .filter(|item| item["call_id"] == "call_plan")
            .cloned()
            .collect::<Vec<_>>(),
        tool_pair,
    );
    let mut projected_reasoning = reasoning["item"].clone();
    projected_reasoning["encrypted_content"] = Value::Null;
    for expected in [&projected_reasoning, &assistant["item"]] {
        assert_eq!(
            input.iter().find(|item| item["id"] == expected["id"]),
            Some(expected)
        );
    }
    let mut expected_search = hosted_item.clone();
    expected_search
        .as_object_mut()
        .context("hosted search object")?
        .remove("wire_blocks");
    assert_eq!(
        input.iter().find(|item| item["id"] == "search_saved"),
        Some(&expected_search)
    );
    assert!(
        !request
            .body_json()
            .to_string()
            .contains("model_output_provenance")
    );

    let resumed_contents = std::fs::read_to_string(&rollout_path)?;
    assert!(
        resumed_contents.starts_with(&saved_contents),
        "resume must append without rewriting saved history"
    );
    let resumed_items = persisted_response_items(&resumed_contents)?;
    assert_eq!(&resumed_items[..saved_items.len()], saved_items.as_slice());
    let legacy = resumed_items
        .iter()
        .find(|item| item.id().is_some_and(|id| id.as_str() == "rs_original"))
        .context("legacy reasoning output")?;
    assert_eq!(
        legacy
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.model_output_provenance.as_ref()),
        None,
    );
    let latest = resumed_items
        .iter()
        .find(|item| item.id().is_some_and(|id| id.as_str() == "msg_resumed"))
        .context("resumed output")?;
    assert_eq!(
        latest
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.model_output_provenance.as_ref()),
        Some(&ModelOutputProvenance {
            provider: Some("rig-responses-resumed".into()),
            model: Some("rig-resumed-model".into()),
            ..original_provenance
        }),
    );
    Ok(())
}

// Fork (nuwax-codex): the compaction trim-retry loop must engage on the
// BRIDGE wires too. Chat-wire providers have no native transport (core
// rejects them Fatal without a bridge feature), so a completed turn here is
// itself structural proof the rig bridge served the request; the mock
// additionally asserts chat-completions-shaped request paths and bodies.
// The rejected compaction request carries the vendor 400 body through the
// bridge's error mapping into the shared context-window classifier.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_chat_compact_recovers_from_context_window_rejection() -> Result<()> {
    use codex_core::compact::SUMMARIZATION_PROMPT;
    use wiremock::matchers::method;
    use wiremock::matchers::path_regex;

    fn chat_sse(text: &str, total_tokens: i64) -> wiremock::ResponseTemplate {
        let first = json!({
            "id": "chatcmpl-x", "object": "chat.completion.chunk", "created": 1,
            "model": "server-model",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
        });
        let final_chunk = json!({
            "id": "chatcmpl-x", "object": "chat.completion.chunk", "created": 1,
            "model": "server-model",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": total_tokens}
        });
        let body = format!("data: {first}\n\ndata: {final_chunk}\n\ndata: [DONE]\n\n");
        responses::sse_response(body)
    }

    let server = MockServer::start().await;
    // Mount in chronological order; each mock exhausts after one hit, so the
    // next request falls through to the next mock (wiremock matches by
    // stable priority = registration order).
    let rejection = wiremock::ResponseTemplate::new(400).set_body_json(json!({
        "error": {
            "code": "context_length_exceeded",
            "message": "This model's maximum context length is 8192 tokens.",
        }
    }));
    let sequence = [
        chat_sse("FIRST_REPLY", /*total_tokens*/ 70_000),
        chat_sse("SECOND_REPLY", /*total_tokens*/ 330_000),
        rejection,
        chat_sse("AUTO_SUMMARY", /*total_tokens*/ 200),
        chat_sse("FINAL_REPLY", /*total_tokens*/ 120),
    ];
    let request_log = ChatRequestLog::default();
    for template in sequence {
        wiremock::Mock::given(method("POST"))
            .and(path_regex(".*/chat/completions$"))
            .and(request_log.clone())
            .respond_with(template)
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }

    let provider = ModelProviderInfo {
        name: "rig-chat".into(),
        wire_api: WireApi::Chat,
        ..bridged_responses_provider(&server)
    };
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
            config.model_auto_compact_token_limit = Some(200_000);
            config.compact_prompt = Some(SUMMARIZATION_PROMPT.to_string());
        })
        .build_with_auto_env(&server)
        .await
        .context("build")?;
    test.submit_text_turn("token limit start").await?;
    test.submit_text_turn("token limit push").await?;
    test.submit_text_turn("post auto follow-up").await?;

    let requests = request_log.requests();
    let paths: Vec<String> = requests.iter().map(|r| r.url.path().to_string()).collect();
    assert_eq!(
        paths,
        vec!["/v1/chat/completions".to_string(); 5],
        "every request must be chat-completions shaped (bridge wire)"
    );
    let body = |index: usize| {
        serde_json::from_slice::<Value>(&requests[index].body).context("request body json")
    };
    // Request 3 is the rejected compaction; request 4 is the trimmed retry.
    let rejected_messages = body(2)?["messages"]
        .as_array()
        .context("rejected request messages")?
        .len();
    let retried_messages = body(3)?["messages"]
        .as_array()
        .context("retried request messages")?
        .len();
    assert!(
        retried_messages < rejected_messages,
        "retry must shed history: {retried_messages} !< {rejected_messages}"
    );
    // The compaction prompt's distinctive template header — the base
    // instructions also ride along in every request, so a generic substring
    // like "summarize" would match ordinary turns too.
    assert!(
        body(2)?
            .to_string()
            .contains("CONTEXT CHECKPOINT COMPACTION"),
        "rejected request is the compaction call"
    );
    assert!(
        body(3)?
            .to_string()
            .contains("CONTEXT CHECKPOINT COMPACTION"),
        "retried request is still the compaction call"
    );
    assert!(
        body(4)?.to_string().contains("post auto follow-up"),
        "the follow-up turn must run after recovery"
    );
    Ok(())
}

#[derive(Clone, Default)]
struct ChatRequestLog(std::sync::Arc<std::sync::Mutex<Vec<wiremock::Request>>>);

impl ChatRequestLog {
    fn requests(&self) -> Vec<wiremock::Request> {
        self.0
            .lock()
            .expect("read captured compaction requests")
            .clone()
    }
}

impl wiremock::Match for ChatRequestLog {
    fn matches(&self, request: &wiremock::Request) -> bool {
        self.0
            .lock()
            .expect("capture compaction request")
            .push(request.clone());
        true
    }
}

/// Core binds an API-key reasoning envelope to its actual immutable credential
/// instance and sends its visible reasoning on the real tool continuation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_credential_instance_preserves_reasoning_content_on_tool_continuation() -> Result<()> {
    use std::sync::Arc;
    use std::sync::Mutex;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let arguments = json!({"plan":[{"step":"retain reasoning", "status":"completed"}]}).to_string();
    let frames = [
        vec![
            json!({"id":"chatcmpl-tool", "object":"chat.completion.chunk", "created":1, "model":"deepseek-test", "choices":[{"index":0, "delta":{"role":"assistant", "reasoning_content":"visible planning thought"}, "finish_reason":null}]}),
            json!({"id":"chatcmpl-tool", "object":"chat.completion.chunk", "created":1, "model":"deepseek-test", "choices":[{"index":0, "delta":{"tool_calls":[{"index":0, "id":"call_plan", "type":"function", "function":{"name":"update_plan", "arguments":arguments}}]}, "finish_reason":null}]}),
            json!({"id":"chatcmpl-tool", "object":"chat.completion.chunk", "created":1, "model":"deepseek-test", "choices":[{"index":0, "delta":{}, "finish_reason":"tool_calls"}], "usage":{"prompt_tokens":4, "completion_tokens":2, "total_tokens":6}}),
        ],
        vec![
            json!({"id":"chatcmpl-final", "object":"chat.completion.chunk", "created":1, "model":"deepseek-test", "choices":[{"index":0, "delta":{"role":"assistant", "content":"done"}, "finish_reason":null}]}),
            json!({"id":"chatcmpl-final", "object":"chat.completion.chunk", "created":1, "model":"deepseek-test", "choices":[{"index":0, "delta":{}, "finish_reason":"stop"}], "usage":{"prompt_tokens":5, "completion_tokens":1, "total_tokens":6}}),
        ],
    ];
    let requests = Arc::new(Mutex::new(Vec::<wiremock::Request>::new()));
    for frames in frames {
        let body = frames
            .into_iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n";
        let captured = Arc::clone(&requests);
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(move |request: &wiremock::Request| {
                captured
                    .lock()
                    .expect("request capture")
                    .push(request.clone());
                true
            })
            .respond_with(responses::sse_response(body))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let provider = ModelProviderInfo {
        name: "DeepSeek test".into(),
        provider_id: Some("deepseek-selector-test".into()),
        wire_api: WireApi::Chat,
        requires_openai_auth: true,
        // PATH is an existing non-secret configuration selector. No key value
        // enters provenance and this test never mutates the process environment.
        env_http_headers: Some([("x-test-selector".into(), "PATH".into())].into()),
        ..bridged_responses_provider(&server)
    };
    let test = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("dummy"))
        .with_model_info_override("deepseek-test", |model| {
            model.use_responses_lite = false;
            model.tool_mode = None;
        })
        .with_config(move |config| {
            config.model_provider = provider;
            config.update_plan_enabled = true;
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("update the plan").await?;
    let rollout = test
        .codex
        .rollout_path()
        .context("Chat projection rollout")?;
    test.codex.shutdown_and_wait().await?;
    let requests = requests.lock().expect("request capture").clone();
    assert_eq!(requests.len(), 2);
    let continuation: Value = serde_json::from_slice(&requests[1].body)?;
    let messages = continuation["messages"]
        .as_array()
        .context("Chat messages")?;
    let tool_loop: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["tool_call_id"] == "call_plan"
                || message["tool_calls"]
                    .as_array()
                    .is_some_and(|calls| calls.iter().any(|call| call["id"] == "call_plan"))
        })
        .cloned()
        .collect();
    assert_eq!(
        tool_loop,
        vec![
            json!({"role":"assistant", "reasoning_content":"visible planning thought", "tool_calls":[
                {"id":"call_plan", "type":"function", "function":{"name":"update_plan", "arguments":arguments}}
            ]}),
            json!({"role":"tool", "tool_call_id":"call_plan", "content":"Plan updated"}),
        ]
    );
    assert_eq!(
        requests[1]
            .headers
            .get("authorization")
            .context("actual API-key header")?,
        "Bearer dummy"
    );
    let saved = persisted_response_items(&std::fs::read_to_string(rollout)?)?;
    let reasoning = saved
        .iter()
        .find(|item| {
            matches!(
                &item.item,
                codex_protocol::models::ResponseItem::Reasoning {
                    encrypted_content: Some(_),
                    ..
                }
            )
        })
        .context("stored original reasoning envelope")?;
    let provenance = reasoning
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.model_output_provenance.as_ref())
        .context("captured selector source")?;
    assert_eq!(
        (
            provenance.wire_protocol.as_str(),
            provenance.provider.as_deref(),
            provenance.auth_domain_kind.as_deref()
        ),
        (
            "chat",
            Some("deepseek-selector-test"),
            Some("credentialInstance")
        )
    );
    assert!(provenance.auth_domain.as_ref().is_some_and(|domain| {
        domain.starts_with("credential-instance-v1:") && !domain.contains("dummy")
    }));
    Ok(())
}

/// A generation budget failure must not enter Core's transport retry loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_bridge_output_cap_exhaustion_does_not_resample() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    // Repeat the failure so an accidental retry cannot be hidden by a mock 404.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .respond_with(responses::sse_response(responses::sse(vec![
        responses::ev_response_created("limited"),
        json!({"type":"response.incomplete","response":{"id":"limited","incomplete_details":{"reason":"max_output_tokens"}}}),
    ]))).mount(&server).await;
    let mut provider = bridged_responses_provider(&server);
    provider.max_output_tokens = Some(64);
    provider.request_max_retries = Some(3);
    provider.stream_max_retries = Some(3);
    let test = test_codex()
        .with_config(move |config| config.model_provider = provider)
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("exercise output cap").await?;
    let requests = server
        .received_requests()
        .await
        .context("recorded requests")?;
    let requests: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/responses" && request.method == "POST")
        .collect();
    assert_eq!(
        requests.len(),
        1,
        "the same exhausted cap cannot be automatically retried"
    );
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(body["max_output_tokens"], json!(64));
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
