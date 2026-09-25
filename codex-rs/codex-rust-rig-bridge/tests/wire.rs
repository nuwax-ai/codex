// Test fixtures should fail immediately when setup or captured data is invalid.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "wire/auth_tests.rs"]
mod auth_tests;
#[path = "wire/error_tests.rs"]
mod error_tests;
mod support;

use codex_api::Reasoning;
use codex_api::ResponseEvent;
use codex_api::TextControls;
use codex_api::TextFormat;
use codex_api::TextFormatType;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rust_rig_bridge::RigProtocol;
use pretty_assertions::assert_eq;
use serde_json::json;
use support::IMAGE;
use support::capture;
use support::request;
use support::set_tools;
use support::user;

#[tokio::test]
async fn final_chat_request_preserves_url_headers_role_effort_and_request_id() {
    let mut request = request(vec![
        json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"developer rule"}]}),
        user(),
    ]);
    request.reasoning = Some(Reasoning {
        effort: Some(ReasoningEffort::Medium),
        summary: None,
        context: None,
    });
    let (wire, events, id) = capture(&request, RigProtocol::Chat).await;
    assert_eq!(
        wire["request_line"],
        "POST /v1/chat/completions?existing=a%26b&api-version=version+with+%26+spaces HTTP/1.1"
    );
    assert!(
        wire["headers"]
            .as_str()
            .unwrap()
            .contains("x-gateway-auth: Bearer local-test-dummy")
    );
    assert_eq!(
        wire["body"]["messages"][1],
        json!({"role":"system","content":[{"type":"text","text":"developer rule"}]})
    );
    assert_eq!(wire["body"]["reasoning_effort"], "medium");
    assert_eq!(id.as_deref(), Some("req-local"));
    assert!(events.iter().any(
        |event| matches!(event, ResponseEvent::ServerModel(model) if model == "server-model")
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ResponseEvent::Completed { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn output_schema_survives_first_tool_turn_on_both_protocols() {
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        let mut request = request(vec![user()]);
        set_tools(
            &mut request,
            json!([{"type":"function","name":"lookup","strict":false,"parameters":{"type":"object","properties":{}}}]),
        );
        let schema = json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false});
        request.text = Some(TextControls {
            verbosity: None,
            format: Some(TextFormat {
                r#type: TextFormatType::JsonSchema,
                strict: false,
                name: "answer_shape".into(),
                schema: schema.clone(),
            }),
        });
        request.parallel_tool_calls = false;
        let (wire, _, _) = capture(&request, protocol).await;
        match protocol {
            RigProtocol::Chat => {
                assert_eq!(wire["body"]["tools"][0]["function"]["strict"], false);
                assert_eq!(wire["body"]["store"], false);
                assert_eq!(
                    wire["body"]["response_format"],
                    json!({"type":"json_schema","json_schema":{"name":"answer_shape","strict":false,"schema":schema}})
                );
            }
            RigProtocol::Anthropic => {
                assert_eq!(wire["body"]["output_config"]["format"]["schema"], schema);
                assert_eq!(
                    wire["body"]["tool_choice"]["disable_parallel_tool_use"],
                    true
                );
                assert!(wire["body"].get("parallel_tool_calls").is_none());
                assert_eq!(
                    wire["request_line"],
                    "POST /v1/messages?existing=a%26b&api-version=version+with+%26+spaces HTTP/1.1"
                );
            }
        }
    }
}

#[tokio::test]
async fn function_and_custom_image_results_reach_both_protocols_as_images() {
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        for custom in [false, true] {
            let (call, result) = if custom {
                (
                    json!({"type":"custom_tool_call","name":"exec","call_id":"c1","input":"image"}),
                    json!({"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_image","image_url":IMAGE,"detail":"high"}]}),
                )
            } else {
                (
                    json!({"type":"function_call","name":"view_image","call_id":"c1","arguments":"{}"}),
                    json!({"type":"function_call_output","call_id":"c1","output":[{"type":"input_image","image_url":IMAGE,"detail":"high"}]}),
                )
            };
            let request = request(vec![user(), call, result]);
            let (wire, _, _) = capture(&request, protocol).await;
            let encoded = wire["body"]["messages"].to_string();
            assert!(
                !encoded.contains("\\\"image_url\\\""),
                "image JSON was inlined into text"
            );
            match protocol {
                RigProtocol::Chat => {
                    let messages = wire["body"]["messages"].as_array().unwrap();
                    let result_index = messages
                        .iter()
                        .position(|message| message["role"] == "tool")
                        .unwrap();
                    assert_eq!(messages[result_index]["tool_call_id"], "c1");
                    assert_eq!(
                        messages[result_index + 1]["content"][1]["image_url"],
                        json!({"url":IMAGE,"detail":"high"})
                    );
                }
                RigProtocol::Anthropic => {
                    let results: Vec<_> = wire["body"]["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|message| message["content"].as_array())
                        .flatten()
                        .filter(|part| part["type"] == "tool_result" && part["tool_use_id"] == "c1")
                        .collect();
                    assert_eq!(results.len(), 1);
                    assert_eq!(
                        results[0]["content"],
                        json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}])
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn namespaced_additional_custom_tools_keep_wrapper_and_history_name() {
    let mut request = request(vec![
        user(),
        json!({"type":"additional_tools","role":"developer","tools":[{"type":"namespace","name":"functions","tools":[{"type":"custom","name":"apply_patch","format":{"type":"text"}}]}]}),
        json!({"type":"custom_tool_call","namespace":"functions","name":"apply_patch","call_id":"c1","input":"patch"}),
        json!({"type":"custom_tool_call_output","call_id":"c1","output":"applied"}),
    ]);
    request.instructions.clear();
    let (wire, _, _) = capture(&request, RigProtocol::Chat).await;
    let tool = &wire["body"]["tools"][0]["function"];
    assert_eq!(tool["parameters"]["properties"]["input"]["type"], "string");
    let history = &wire["body"]["messages"][1]["tool_calls"][0]["function"];
    assert_eq!(history["name"], tool["name"]);
    assert_eq!(
        codex_rust_rig_bridge::extract_custom_tool_names(&request),
        [tool["name"].as_str().unwrap().to_string()]
            .into_iter()
            .collect()
    );
}
