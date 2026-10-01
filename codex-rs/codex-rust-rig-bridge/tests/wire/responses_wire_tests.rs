//! Wire-level tests for the Responses same-protocol passthrough: the outbound
//! body must be Codex's `ResponsesApiRequest` serialized verbatim, and the
//! inbound stream must decode through codex-api's Responses decoder under the
//! strict terminal policy. Chat/Anthropic conversions are covered by the
//! sibling suites; nothing here may change them.

use super::support;
use codex_api::Reasoning;
use codex_api::ResponseEvent;
use codex_api::RetryConfig;
use codex_api::TextControls;
use codex_api::TextFormat;
use codex_api::TextFormatType;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::TokenUsage;
use codex_rust_rig_bridge::RigProtocol;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;

/// The passthrough must send the request byte-for-byte as codex serialized
/// it: no role rewriting, message merging, tool filtering, namespace
/// flattening, or reasoning-envelope conversion.
#[tokio::test]
async fn responses_wire_sends_the_request_verbatim() {
    let mut request = support::request(vec![
        support::user(),
        json!({"type":"function_call","name":"lookup","call_id":"call_1","arguments":"{\"q\":\"x\"}"}),
        json!({"type":"function_call_output","call_id":"call_1","output":"found"}),
    ]);
    support::set_tools(
        &mut request,
        json!([
            {"type":"function","name":"lookup","strict":false,"parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}},
            {"type":"namespace","name":"functions","tools":[{"type":"custom","name":"apply_patch","format":{"type":"text"}}]}
        ]),
    );
    request.reasoning = Some(Reasoning {
        effort: Some(ReasoningEffort::High),
        summary: None,
        context: None,
    });
    request.service_tier = Some("auto".into());
    request.prompt_cache_key = Some("thread-cache-key".into());
    request.parallel_tool_calls = false;
    request.include = vec!["reasoning.encrypted_content".into()];
    request.text = Some(TextControls {
        verbosity: None,
        format: Some(TextFormat {
            r#type: TextFormatType::JsonSchema,
            strict: true,
            name: "answer_shape".into(),
            schema: json!({"type":"object"}),
        }),
    });
    let expected = serde_json::to_value(&request).expect("serialize request");
    let (wire, _, _) = support::capture(&request, RigProtocol::Responses).await;
    // Full-body comparison: every modeled field must survive untouched.
    assert_eq!(wire["body"], expected);
    // And spot the fields a Chat conversion would have destroyed.
    assert_eq!(wire["body"]["input"].as_array().map(Vec::len), Some(3));
    assert_eq!(wire["body"]["tool_choice"], "auto");
    assert_eq!(wire["body"]["service_tier"], "auto");
    assert_eq!(wire["body"]["prompt_cache_key"], "thread-cache-key");
    assert_eq!(wire["body"]["parallel_tool_calls"], false);
    assert_eq!(
        wire["body"]["include"],
        json!(["reasoning.encrypted_content"])
    );
    assert_eq!(wire["body"]["tools"][1]["type"], "namespace");
}

/// Endpoint targeting: `{base}/responses` with re-encoded query pairs,
/// bearer auth rebuilt by rig, gateway headers passed through.
#[tokio::test]
async fn responses_wire_targets_responses_endpoint_with_auth_and_query() {
    let request = support::request(vec![support::user()]);
    let (wire, _, request_id) = support::capture(&request, RigProtocol::Responses).await;
    assert_eq!(
        wire["request_line"],
        "POST /v1/responses?existing=a%26b&api-version=version+with+%26+spaces HTTP/1.1"
    );
    let headers = wire["headers"].as_str().expect("headers");
    assert!(
        headers.contains("authorization: Bearer local-test-dummy")
            || headers.contains("Authorization: Bearer local-test-dummy"),
        "rig must rebuild the bearer header: {headers}"
    );
    assert!(
        headers.contains("x-gateway-auth: Bearer local-test-dummy"),
        "gateway headers pass through: {headers}"
    );
    assert_eq!(request_id.as_deref(), Some("req-local"));
}

/// Event fidelity through the shared decoder: created/delta/item/completed,
/// usage, exactly one terminal.
#[tokio::test]
async fn responses_wire_events_preserve_ids_usage_and_single_terminal() {
    let request = support::request(vec![support::user()]);
    let (_, events, _) = support::capture(&request, RigProtocol::Responses).await;
    assert!(
        events.iter().any(|event| matches!(event, ResponseEvent::Created { response_id } if response_id.as_deref() == Some("resp-test")))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ResponseEvent::OutputTextDelta(_)))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ResponseEvent::OutputItemDone(codex_protocol::models::ResponseItem::Message { role, .. }) if role == "assistant"
    )));
    let completed: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, ResponseEvent::Completed { .. }))
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed.len(),
        1,
        "exactly one terminal event, got {events:?}"
    );
    match completed[0] {
        ResponseEvent::Completed { token_usage, .. } => assert_eq!(
            token_usage.clone(),
            Some(TokenUsage {
                input_tokens: 4,
                output_tokens: 1,
                total_tokens: 5,
                ..Default::default()
            })
        ),
        other => panic!("unexpected terminal: {other:?}"),
    }
}

/// Cross-protocol resume (phase 2): history recorded on the Chat/Anthropic
/// wires carries rig replay envelopes; the Responses wire must receive the
/// same history with the envelope cleared to null and visible reasoning kept.
#[tokio::test]
async fn responses_wire_projects_chat_history_envelopes() {
    let request = support::request(vec![
        support::user(),
        json!({
            "type":"reasoning",
            "id":"rsn_chat",
            "summary":[],
            "content":[{"type":"reasoning_text","text":"visible prior thinking"}],
            "encrypted_content":"codex-rig-reasoning-v1:{\"source\":\"chat-source\"}"
        }),
        json!({"type":"function_call","id":"fc_1","name":"lookup","call_id":"call_1","arguments":"{\"q\":\"x\"}"}),
        json!({"type":"function_call_output","call_id":"call_1","output":"seen"}),
        json!({
            "type":"web_search_call", "id":"search_saved", "status":"completed",
            "action":{"type":"search", "query":"saved query"},
            "wire_blocks":[
                {"type":"server_tool_use", "id":"srv_saved", "name":"web_search", "input":{"query":"saved query"}},
                {"type":"web_search_tool_result", "tool_use_id":"srv_saved", "content":[{"type":"web_search_result", "encrypted_content":"vendor-secret"}]}
            ]
        }),
        support::user(),
    ]);
    // Sanity: the serialized request itself still carries the envelope —
    // projection must happen on the wire copy only.
    let serialized = serde_json::to_value(&request).expect("serialize request");
    assert!(
        serialized["input"][1]["encrypted_content"]
            .as_str()
            .is_some_and(|value| value.starts_with("codex-rig-reasoning-v1:"))
    );
    let (wire, events, _) = support::capture(&request, RigProtocol::Responses).await;
    assert!(
        wire["body"]["input"][1]["encrypted_content"].is_null(),
        "envelope must be projected to the ciphertext-less shape: {}",
        wire["body"]["input"][1]
    );
    // Real Rig outputs contain ReasoningText and an empty summary. Serde
    // preserves that content, so projection must only clear the envelope.
    assert_eq!(
        wire["body"]["input"][1],
        json!({
            "type":"reasoning",
            "id":"rsn_chat",
            "summary":[],
            "content":[{"type":"reasoning_text","text":"visible prior thinking"}],
            "encrypted_content":null
        })
    );
    assert_eq!(
        serde_json::to_value(&request).expect("serialize original request"),
        serialized,
        "projection must not change the original request"
    );
    // Tool pairing passes through untouched.
    assert_eq!(wire["body"]["input"][2]["call_id"], "call_1");
    assert_eq!(wire["body"]["input"][3]["output"], "seen");
    assert_eq!(
        wire["body"]["input"][4],
        json!({
            "type":"web_search_call", "id":"search_saved", "status":"completed",
            "action":{"type":"search", "query":"saved query"}
        }),
        "Anthropic replay state must stay out of the Responses wire"
    );
    // The turn still completes normally on the scripted Responses stream.
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ResponseEvent::Completed { .. }))
    );
}

/// A rich stream with reasoning summaries and two parallel tool calls must
/// keep item/call IDs, deltas, and pairing.
#[tokio::test]
async fn responses_wire_reasoning_and_parallel_tool_calls_keep_ids_and_order() {
    let sse = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-rich\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[]}}\n\n",
        "data: {\"type\":\"response.reasoning_summary_part.added\",\"item_id\":\"rs_1\",\"summary_index\":0}\n\n",
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs_1\",\"summary_index\":0,\"delta\":\"thinking\"}\n\n",
        "data: {\"type\":\"response.reasoning_summary_text.done\",\"item_id\":\"rs_1\",\"summary_index\":0,\"text\":\"thinking\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[]}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_a\",\"name\":\"lookup\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_b\",\"name\":\"inspect\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"q\\\":\\\"a\\\"}\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_2\",\"delta\":\"{\\\"q\\\":\\\"b\\\"}\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_a\",\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\\\"a\\\"}\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_b\",\"name\":\"inspect\",\"arguments\":\"{\\\"q\\\":\\\"b\\\"}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-rich\",\"usage\":{\"input_tokens\":10,\"output_tokens\":20,\"total_tokens\":30}}}\n\n",
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { support::serve_payload(&listener, sse).await });
    let provider = codex_api::Provider {
        name: "local test".into(),
        base_url: format!("http://{address}/v1"),
        query_params: None,
        headers: http::HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(3),
        max_output_tokens: None,
        hosted_results_replay: None,
    };
    let auth: codex_api::SharedAuthProvider = std::sync::Arc::new(support::DummyAuth);
    let mut stream = codex_rust_rig_bridge::stream_via_rig(
        &support::request(vec![support::user()]),
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Responses,
        Duration::from_secs(3),
    )
    .await
    .expect("start stream");
    let mut events = Vec::new();
    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }
    server.await.unwrap();

    let mut call_ids = Vec::new();
    for event in &events {
        if let ResponseEvent::OutputItemDone(codex_protocol::models::ResponseItem::FunctionCall {
            call_id,
            name,
            arguments,
            ..
        }) = event
        {
            call_ids.push(call_id.clone());
            assert!(matches!(
                (name.as_str(), arguments.as_str()),
                ("lookup", "{\"q\":\"a\"}") | ("inspect", "{\"q\":\"b\"}")
            ));
        }
    }
    assert_eq!(call_ids, vec!["call_a".to_string(), "call_b".to_string()]);
    assert!(events.iter().any(|event| matches!(
        event,
        ResponseEvent::ReasoningSummaryDelta { delta, .. } if delta == "thinking"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ResponseEvent::ReasoningSummaryDone { item_id, text, .. } if item_id == "rs_1" && text == "thinking"
    )));
    let first_reasoning = events
        .iter()
        .position(|event| matches!(event, ResponseEvent::ReasoningSummaryDelta { .. }))
        .expect("reasoning delta");
    let first_call = events
        .iter()
        .position(|event| {
            matches!(
                event,
                ResponseEvent::OutputItemDone(
                    codex_protocol::models::ResponseItem::FunctionCall { .. }
                )
            )
        })
        .expect("function call");
    assert!(
        first_reasoning < first_call,
        "reasoning precedes tool calls"
    );
}

/// Second-turn history (assistant function_call + user function_call_output)
/// must round-trip verbatim with call_id pairing intact.
#[tokio::test]
async fn responses_wire_second_turn_replays_tool_results_verbatim() {
    let request = support::request(vec![
        support::user(),
        json!({"type":"function_call","id":"fc_9","name":"lookup","call_id":"call_9","arguments":"{\"q\":\"x\"}"}),
        json!({"type":"function_call_output","call_id":"call_9","output":"result-payload"}),
    ]);
    let (wire, _, _) = support::capture(&request, RigProtocol::Responses).await;
    assert_eq!(
        wire["body"]["input"][2],
        json!({"type":"function_call_output","call_id":"call_9","output":"result-payload"})
    );
    assert_eq!(
        wire["body"]["input"][1],
        json!({"type":"function_call","id":"fc_9","name":"lookup","call_id":"call_9","arguments":"{\"q\":\"x\"}"})
    );
}

/// Compatible gateways may append a Chat-style `[DONE]` sentinel (GLM does);
/// the strict terminal stays response.completed.
#[tokio::test]
async fn responses_wire_tolerates_trailing_done_sentinel() {
    let sse = format!(
        "{RESPONSES}{DONE}",
        RESPONSES = support::RESPONSES_SSE,
        DONE = "data: [DONE]\n\n"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { support::serve_payload(&listener, &sse).await });
    let provider = codex_api::Provider {
        name: "local test".into(),
        base_url: format!("http://{address}/v1"),
        query_params: None,
        headers: http::HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(3),
        max_output_tokens: None,
        hosted_results_replay: None,
    };
    let auth: codex_api::SharedAuthProvider = std::sync::Arc::new(support::DummyAuth);
    let mut stream = codex_rust_rig_bridge::stream_via_rig(
        &support::request(vec![support::user()]),
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Responses,
        Duration::from_secs(3),
    )
    .await
    .expect("start stream");
    use futures::StreamExt;
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }
    server.await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ResponseEvent::Completed { .. }))
            .count(),
        1
    );
}
