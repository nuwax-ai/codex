//! Bridge-level live suites: the same MiMo scenarios run through both the
//! genai bridge and the rig bridge so their event streams can be compared
//! (A/B) while asserting codex's protocol invariants on real model output.
//!
//! Skipped entirely when `MIMO_API_KEY` is not configured.

use codex_api::ResponsesApiRequest;
use codex_live_tests::live_config;
use codex_live_tests::run_turn_genai;
use codex_live_tests::run_turn_rig;
use codex_live_tests::user_message;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use genai::adapter::AdapterKind;
use std::sync::Arc;

fn base_request(model: &str, instructions: &str, prompt: &str) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: model.to_string(),
        instructions: instructions.into(),
        input: vec![user_message(prompt)],
        tools: None,
        tool_choice: "auto".into(),
        parallel_tool_calls: true,
        reasoning: None,
        store: false,
        stream: true,
        stream_options: None,
        include: vec![],
        service_tier: None,
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    }
}

// ================================================================
// genai bridge
// ================================================================

#[tokio::test]
async fn genai_chat_round_trip_streams_text_reasoning_and_usage() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = base_request(
        &cfg.model,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话解释什么是斐波那契数列。",
    );

    let events = run_turn_genai(
        &request,
        &cfg.base_url,
        &cfg.api_key,
        AdapterKind::OpenAI,
        "genai-chat",
    )
    .await;

    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected at least one OutputTextDelta"
    );
    println!(
        "[summary] text_chars={} reasoning_chars={}",
        codex_live_tests::text_len(&events),
        codex_live_tests::reasoning_len(&events)
    );
    codex_live_tests::assert_completed_with_usage(&events, "genai chat round trip");
    codex_live_tests::assert_reasoning_before_message(&events, "genai chat round trip");
    assert_eq!(
        codex_live_tests::end_turn_of(&events),
        Some(true),
        "plain chat turn should end with end_turn=true"
    );
}

#[tokio::test]
async fn genai_chat_with_reasoning_effort_low_is_accepted() {
    let Some(cfg) = live_config() else {
        return;
    };
    let mut request = base_request(
        &cfg.model,
        "You are a helpful assistant.",
        "1+1 等于几？直接回答。",
    );
    request.reasoning = Some(codex_api::Reasoning {
        effort: Some(codex_protocol::openai_models::ReasoningEffort::Low),
        summary: None,
        context: None,
    });

    let events = run_turn_genai(
        &request,
        &cfg.base_url,
        &cfg.api_key,
        AdapterKind::OpenAI,
        "genai-effort",
    )
    .await;

    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected an answer to the question"
    );
    codex_live_tests::assert_completed_with_usage(&events, "genai reasoning effort low");
}

#[tokio::test]
async fn genai_tool_call_round_trip_with_tool_result_replay() {
    let Some(cfg) = live_config() else {
        return;
    };
    let mut request = base_request(
        &cfg.model,
        "You are a helpful assistant.",
        "北京今天天气怎么样？请务必调用 get_weather 工具查询，不要凭空回答。",
    );
    request.tools = Some(weather_tools());
    request.parallel_tool_calls = false;

    let turn1 = run_turn_genai(
        &request,
        &cfg.base_url,
        &cfg.api_key,
        AdapterKind::OpenAI,
        "genai-tool-t1",
    )
    .await;
    codex_live_tests::assert_completed_with_usage(&turn1, "genai tool turn 1");
    codex_live_tests::assert_reasoning_before_message(&turn1, "genai tool turn 1");
    codex_live_tests::assert_tool_deltas_reassemble(&turn1, "genai tool turn 1");
    assert_eq!(
        codex_live_tests::end_turn_of(&turn1),
        Some(false),
        "tool-call turn should end with end_turn=false"
    );

    let function_call = turn1
        .iter()
        .find_map(|e| match e {
            codex_api::ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) => Some((name.clone(), arguments.clone(), call_id.clone())),
            _ => None,
        })
        .expect("model should emit a get_weather function call");
    println!(
        "[summary] function_call name={} call_id={} arguments={}",
        function_call.0, function_call.2, function_call.1
    );
    assert_eq!(function_call.0, "get_weather");
    let args: serde_json::Value =
        serde_json::from_str(&function_call.1).expect("tool arguments are valid JSON");
    assert!(
        args.get("city").is_some(),
        "tool arguments should contain the city field: {args}"
    );

    // Turn 2: replay history plus the tool result — the stateless
    // multi-turn shape codex uses for chat providers.
    let mut input = request.input.clone();
    let (_, arguments, call_id) = function_call;
    input.push(ResponseItem::FunctionCall {
        id: None,
        name: "get_weather".into(),
        namespace: None,
        arguments,
        encrypted_function_args: None,
        call_id: call_id.clone(),
        internal_chat_message_metadata_passthrough: None,
    });
    input.push(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                r#"{"city":"北京","condition":"晴","temp_c":23}"#.into(),
            ),
            success: Some(true),
        },
        internal_chat_message_metadata_passthrough: None,
    });
    request.input = input;

    let turn2 = run_turn_genai(
        &request,
        &cfg.base_url,
        &cfg.api_key,
        AdapterKind::OpenAI,
        "genai-tool-t2",
    )
    .await;
    assert!(
        codex_live_tests::text_len(&turn2) > 0,
        "expected the model to answer with text after the tool result"
    );
    println!("[summary] final_answer_chars={}", codex_live_tests::text_len(&turn2));
    codex_live_tests::assert_completed_with_usage(&turn2, "genai tool turn 2");
}

#[tokio::test]
async fn genai_anthropic_protocol_chat_round_trip() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = base_request(
        &cfg.model,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话说明二分查找的思想。",
    );

    let events = run_turn_genai(
        &request,
        &cfg.anthropic_base_url,
        &cfg.api_key,
        AdapterKind::Anthropic,
        "genai-anthropic",
    )
    .await;

    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected text output through the Anthropic protocol"
    );
    println!(
        "[summary] anthropic text_chars={} reasoning_chars={}",
        codex_live_tests::text_len(&events),
        codex_live_tests::reasoning_len(&events)
    );
    codex_live_tests::assert_completed_with_usage(&events, "genai anthropic round trip");
    codex_live_tests::assert_reasoning_before_message(&events, "genai anthropic round trip");
}

// ================================================================
// rig bridge (same scenarios, same invariants)
// ================================================================

#[tokio::test]
async fn rig_chat_round_trip_streams_text_reasoning_and_usage() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = base_request(
        &cfg.model,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话解释什么是斐波那契数列。",
    );

    let events = run_turn_rig(&request, &cfg.base_url, &cfg.api_key, "rig-chat").await;

    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected at least one OutputTextDelta"
    );
    println!(
        "[summary] text_chars={} reasoning_chars={}",
        codex_live_tests::text_len(&events),
        codex_live_tests::reasoning_len(&events)
    );
    codex_live_tests::assert_completed_with_usage(&events, "rig chat round trip");
    codex_live_tests::assert_reasoning_before_message(&events, "rig chat round trip");
    assert_eq!(
        codex_live_tests::end_turn_of(&events),
        Some(true),
        "plain chat turn should end with end_turn=true"
    );
}

#[tokio::test]
async fn rig_tool_call_round_trip_with_tool_result_replay() {
    let Some(cfg) = live_config() else {
        return;
    };
    let mut request = base_request(
        &cfg.model,
        "You are a helpful assistant.",
        "北京今天天气怎么样？请务必调用 get_weather 工具查询，不要凭空回答。",
    );
    request.tools = Some(weather_tools());
    request.parallel_tool_calls = false;

    let turn1 = run_turn_rig(&request, &cfg.base_url, &cfg.api_key, "rig-tool-t1").await;
    codex_live_tests::assert_completed_with_usage(&turn1, "rig tool turn 1");
    codex_live_tests::assert_reasoning_before_message(&turn1, "rig tool turn 1");
    assert_eq!(
        codex_live_tests::end_turn_of(&turn1),
        Some(false),
        "tool-call turn should end with end_turn=false"
    );

    let function_call = turn1
        .iter()
        .find_map(|e| match e {
            codex_api::ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) => Some((name.clone(), arguments.clone(), call_id.clone())),
            _ => None,
        })
        .expect("model should emit a get_weather function call");
    println!(
        "[summary] function_call name={} call_id={} arguments={}",
        function_call.0, function_call.2, function_call.1
    );
    assert_eq!(function_call.0, "get_weather");

    let mut input = request.input.clone();
    let (_, arguments, call_id) = function_call;
    input.push(ResponseItem::FunctionCall {
        id: None,
        name: "get_weather".into(),
        namespace: None,
        arguments,
        encrypted_function_args: None,
        call_id: call_id.clone(),
        internal_chat_message_metadata_passthrough: None,
    });
    input.push(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                r#"{"city":"北京","condition":"晴","temp_c":23}"#.into(),
            ),
            success: Some(true),
        },
        internal_chat_message_metadata_passthrough: None,
    });
    request.input = input;

    let turn2 = run_turn_rig(&request, &cfg.base_url, &cfg.api_key, "rig-tool-t2").await;
    assert!(
        codex_live_tests::text_len(&turn2) > 0,
        "expected the model to answer with text after the tool result"
    );
    println!("[summary] final_answer_chars={}", codex_live_tests::text_len(&turn2));
    codex_live_tests::assert_completed_with_usage(&turn2, "rig tool turn 2");
}

#[tokio::test]
async fn rig_anthropic_protocol_chat_round_trip() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = base_request(
        &cfg.model,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话说明二分查找的思想。",
    );

    let events = run_turn_rig(
        &request,
        &cfg.anthropic_base_url,
        &cfg.api_key,
        "rig-anthropic",
    )
    .await;

    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected text output through the Anthropic protocol"
    );
    println!(
        "[summary] anthropic text_chars={} reasoning_chars={}",
        codex_live_tests::text_len(&events),
        codex_live_tests::reasoning_len(&events)
    );
    codex_live_tests::assert_completed_with_usage(&events, "rig anthropic round trip");
    codex_live_tests::assert_reasoning_before_message(&events, "rig anthropic round trip");
}

fn weather_tools() -> codex_api::ResponsesApiTools {
    let tools_json = r#"[{
        "type": "function",
        "name": "get_weather",
        "description": "Get the current weather for a city",
        "parameters": {
            "type": "object",
            "properties": {
                "city": {"type": "string", "description": "City name, e.g. 北京"}
            },
            "required": ["city"]
        },
        "strict": false
    }]"#;
    let raw = serde_json::value::RawValue::from_string(tools_json.to_string())
        .expect("valid tool json");
    codex_api::ResponsesApiTools::from(Arc::from(raw))
}
