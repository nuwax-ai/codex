use super::error_tests::provider;
use super::support;
use codex_api::Reasoning;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn anthropic_tool_turn() -> String {
    let mut frames = vec![
        json!({"type":"message_start","message":{"id":"msg-history","type":"message","role":"assistant","content":[],"model":"review-model","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":0}}}),
    ];
    for (index, text, signature) in [
        (0, "first", "signature-one"),
        (1, "second", "signature-two"),
    ] {
        frames.extend([
            json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":text}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":signature}}),
            json!({"type":"content_block_stop","index":index}),
        ]);
    }
    frames.extend([
        json!({"type":"content_block_start","index":2,"content_block":{"type":"redacted_thinking","data":"redacted-original"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"call-history","name":"lookup","input":{}}}),
        json!({"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"hello\"}"}}),
        json!({"type":"content_block_stop","index":3}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":20}}),
        json!({"type":"message_stop"}),
    ]);
    frames
        .into_iter()
        .map(|frame| {
            format!(
                "event: {}\ndata: {frame}\n\n",
                frame["type"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn actual_anthropic_second_request_preserves_all_thinking_and_tool_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        support::serve_payload(&listener, &anthropic_tool_turn()).await;
        support::serve_payload(&listener, support::ANTHROPIC_SSE).await
    });
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}}}}]),
    );
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut first = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    while let Some(event) = first.next().await {
        if let ResponseEvent::OutputItemDone(item) = event.unwrap() {
            request.input.push(item);
        }
    }
    let mut result: ResponseItem = serde_json::from_value(
        json!({"type":"function_call_output","call_id":"call-history","output":"lookup failed"}),
    )
    .unwrap();
    if let ResponseItem::FunctionCallOutput { output, .. } = &mut result {
        output.success = Some(false);
    }
    request.input.push(result);
    let mut second = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    while let Some(event) = second.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    assert_eq!(
        wire["body"]["messages"][1],
        json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"first","signature":"signature-one"},
            {"type":"thinking","thinking":"second","signature":"signature-two"},
            {"type":"redacted_thinking","data":"redacted-original"},
            {"type":"tool_use","id":"call-history","name":"lookup","input":{"query":"hello"}}
        ]})
    );
    assert_eq!(
        wire["body"]["messages"][2],
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call-history","content":[{"type":"text","text":"lookup failed"}],"is_error":true}]})
    );
}

#[tokio::test]
async fn standalone_external_output_is_attributed_user_content_on_both_wires() {
    let request = support::request(vec![
        support::user(),
        json!({"type":"function_call_output","name":"notification","namespace":"functions","output":"background task completed"}),
    ]);
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        let (wire, _, _) = support::capture(&request, protocol).await;
        let messages = wire["body"]["messages"].as_array().unwrap();
        let event = messages.last().unwrap();
        assert_eq!(event["role"], "user");
        let encoded = event.to_string();
        assert!(encoded.contains("notification") && encoded.contains("background task completed"));
        assert!(!encoded.contains("tool_result") && !encoded.contains("tool_call_id"));
    }
}

#[tokio::test]
async fn none_effort_disables_anthropic_thinking_and_default_tier_is_standard() {
    let mut request = support::request(vec![support::user()]);
    request.reasoning = Some(Reasoning {
        effort: Some(ReasoningEffort::None),
        summary: None,
        context: None,
    });
    request.service_tier = Some("default".into());
    let (wire, _, _) = support::capture(&request, RigProtocol::Anthropic).await;
    assert_eq!(wire["body"]["thinking"], json!({"type":"disabled"}));
    assert_eq!(wire["body"]["service_tier"], "standard_only");
    assert_eq!(wire["body"].get("output_config"), None);
}

#[tokio::test]
async fn tool_success_flag_is_preserved_and_absent_flags_remain_absent() {
    for success in [Some(true), None] {
        let mut request = support::request(vec![
            support::user(),
            json!({"type":"custom_tool_call","name":"exec","call_id":"c1","input":"cmd"}),
            json!({"type":"custom_tool_call_output","call_id":"c1","output":"done"}),
        ]);
        if let ResponseItem::CustomToolCallOutput { output, .. } = &mut request.input[2] {
            output.success = success;
        }
        let (wire, _, _) = support::capture(&request, RigProtocol::Anthropic).await;
        let result = &wire["body"]["messages"][2]["content"][0];
        assert_eq!(
            result.get("is_error").cloned(),
            success.map(|value| Value::Bool(!value))
        );
    }
}

#[tokio::test]
async fn multipart_tool_text_has_one_aggregate_budget() {
    for text in ["word ".repeat(12_000), "a".repeat(39_200)] {
        let request = support::request(vec![
            support::user(),
            json!({"type":"function_call","name":"lookup","call_id":"c1","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"c1","output":[{"type":"input_text","text":text},{"type":"input_text","text":"final error: permission denied"}]}),
        ]);
        for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
            let (wire, _, _) = support::capture(&request, protocol).await;
            let messages = wire["body"]["messages"].as_array().unwrap();
            let result = messages.last().unwrap().to_string();
            assert!(codex_utils_string::approx_token_count(&result) < 10_000);
            assert!(result.contains("truncated"));
        }
    }
}

#[tokio::test]
async fn schema_constraints_optional_fields_effort_and_tool_strict_coexist() {
    let mut request = support::request(vec![support::user()]);
    let schema = json!({"type":"object","properties":{"required_value":{"type":"integer","minimum":10},"optional_note":{"type":"string","maxLength":20}},"required":["required_value"],"additionalProperties":false});
    request.text = Some(codex_api::TextControls {
        verbosity: None,
        format: Some(codex_api::TextFormat {
            r#type: codex_api::TextFormatType::JsonSchema,
            strict: false,
            schema: schema.clone(),
            name: "original-name".into(),
        }),
    });
    request.reasoning = Some(Reasoning {
        effort: Some(ReasoningEffort::High),
        summary: None,
        context: None,
    });
    support::set_tools(
        &mut request,
        json!([{"type":"function","name":"lookup","strict":true,"parameters":{"type":"object","properties":{},"additionalProperties":false}}]),
    );
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        let (wire, _, _) = support::capture(&request, protocol).await;
        match protocol {
            RigProtocol::Chat => {
                assert_eq!(
                    wire["body"]["response_format"],
                    json!({"type":"json_schema","json_schema":{"name":"original-name","strict":false,"schema":schema}})
                );
                assert_eq!(wire["body"]["tools"][0]["function"]["strict"], true);
                assert_eq!(wire["body"]["reasoning_effort"], "high");
            }
            RigProtocol::Anthropic => {
                assert_eq!(
                    wire["body"]["output_config"],
                    json!({"effort":"high","format":{"type":"json_schema","schema":schema}})
                );
                assert_eq!(wire["body"]["tools"][0]["strict"], true);
                for key in [
                    "response_format",
                    "reasoning_effort",
                    "parallel_tool_calls",
                    "store",
                    "prompt_cache_key",
                ] {
                    assert!(
                        wire["body"].get(key).is_none(),
                        "OpenAI field leaked: {key}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn unsupported_content_fails_before_network_without_logging_payloads() {
    let provider = provider("127.0.0.1:1".parse().unwrap());
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        for part in [
            json!({"type":"input_image","file_id":"private-file"}),
            json!({"type":"input_audio","audio_url":"data:audio/wav;base64,private-audio"}),
        ] {
            let request = support::request(vec![
                support::user(),
                json!({"type":"message","role":"user","content":[part]}),
            ]);
            let result = stream_via_rig(
                &request,
                &provider,
                &auth,
                http::HeaderMap::new(),
                protocol,
                Duration::from_secs(1),
            )
            .await;
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("unsupported input accepted"),
            };
            assert!(matches!(error, codex_api::ApiError::InvalidRequest { .. }));
            assert!(!error.to_string().contains("private-"));
        }
        let mut request = support::request(vec![support::user()]);
        request.stream = false;
        assert!(matches!(
            stream_via_rig(
                &request,
                &provider,
                &auth,
                http::HeaderMap::new(),
                protocol,
                Duration::from_secs(1)
            )
            .await,
            Err(codex_api::ApiError::InvalidRequest { .. })
        ));
    }
}

#[tokio::test]
async fn provider_completion_ids_survive_sdk_normalization() {
    let request = support::request(vec![support::user()]);
    for (protocol, expected) in [
        (RigProtocol::Chat, "chatcmpl-test"),
        (RigProtocol::Anthropic, "msg-test"),
    ] {
        let (_, events, _) = support::capture(&request, protocol).await;
        let ids: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::Completed { response_id, .. } => Some(response_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![expected]);
    }
}
