//! Bridge-level live matrix: scenarios run through BOTH bridges against
//! EVERY configured vendor (`LIVE_VENDORS`, default mimo), asserting codex's
//! protocol invariants on real model streams. Tests are generated per
//! vendor × bridge so nextest reports each combination separately; vendors
//! without an Anthropic gateway skip that scenario.
//!
//! Adding a vendor: add it to the `matrix!` list below + set its
//! `LIVE_<NAME>_*` variables in `.env.local` — nothing else.

use codex_api::ResponsesApiRequest;
use codex_api::ResponseEvent;
use codex_live_tests::anthropic_url_or_skip;
use codex_live_tests::run_turn;
use codex_live_tests::user_message;
use codex_live_tests::Bridge;
use codex_live_tests::LiveConfig;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use std::sync::Arc;

// ================================================================
// Scenario bodies (shared by every vendor × bridge combination)
// ================================================================

async fn scenario_chat(cfg: &LiveConfig, bridge: Bridge) {
    let request = base_request(
        cfg,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话解释什么是斐波那契数列。",
    );
    let events = run_turn(cfg, &cfg.base_url, bridge, &request, "chat").await;

    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected at least one OutputTextDelta"
    );
    println!(
        "[summary] text_chars={} reasoning_chars={}",
        codex_live_tests::text_len(&events),
        codex_live_tests::reasoning_len(&events)
    );
    let ctx = format!("{}/{} chat", cfg.vendor, bridge.name());
    codex_live_tests::assert_completed_with_usage(&events, &ctx);
    codex_live_tests::assert_reasoning_before_message(&events, &ctx);
    assert_eq!(
        codex_live_tests::end_turn_of(&events),
        Some(true),
        "{ctx}: plain chat turn should end with end_turn=true"
    );
}

async fn scenario_effort_low(cfg: &LiveConfig, bridge: Bridge) {
    let mut request = base_request(
        cfg,
        "You are a helpful assistant.",
        "1+1 等于几？直接回答。",
    );
    request.reasoning = Some(codex_api::Reasoning {
        effort: Some(codex_protocol::openai_models::ReasoningEffort::Low),
        summary: None,
        context: None,
    });
    let events = run_turn(cfg, &cfg.base_url, bridge, &request, "effort").await;
    assert!(
        codex_live_tests::text_len(&events) > 0,
        "expected an answer to the question"
    );
    codex_live_tests::assert_completed_with_usage(
        &events,
        &format!("{}/{} effort-low", cfg.vendor, bridge.name()),
    );
}

async fn scenario_tool_round_trip(cfg: &LiveConfig, bridge: Bridge) {
    let ctx = format!("{}/{} tool", cfg.vendor, bridge.name());
    let mut request = base_request(
        cfg,
        "You are a helpful assistant.",
        "北京今天天气怎么样？请务必调用 get_weather 工具查询，不要凭空回答。",
    );
    request.tools = Some(weather_tools());
    request.parallel_tool_calls = false;

    let turn1 = run_turn(cfg, &cfg.base_url, bridge, &request, "tool-t1").await;
    codex_live_tests::assert_completed_with_usage(&turn1, &ctx);
    codex_live_tests::assert_reasoning_before_message(&turn1, &ctx);
    codex_live_tests::assert_tool_deltas_reassemble(&turn1, &ctx);
    assert_eq!(
        codex_live_tests::end_turn_of(&turn1),
        Some(false),
        "{ctx}: tool-call turn should end with end_turn=false"
    );

    let function_call = extract_function_call(&turn1)
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

    let turn2 = run_turn(cfg, &cfg.base_url, bridge, &request, "tool-t2").await;
    assert!(
        codex_live_tests::text_len(&turn2) > 0,
        "{ctx}: expected the model to answer with text after the tool result"
    );
    println!(
        "[summary] final_answer_chars={}",
        codex_live_tests::text_len(&turn2)
    );
    codex_live_tests::assert_completed_with_usage(&turn2, &ctx);
}

async fn scenario_anthropic(cfg: &LiveConfig, bridge: Bridge) {
    let Some(anthropic_url) = anthropic_url_or_skip(cfg) else {
        return;
    };
    let request = base_request(
        cfg,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话说明二分查找的思想。",
    );
    let events = run_turn(cfg, &anthropic_url, bridge, &request, "anthropic").await;

    let ctx = format!("{}/{} anthropic", cfg.vendor, bridge.name());
    assert!(
        codex_live_tests::text_len(&events) > 0,
        "{ctx}: expected text output through the Anthropic protocol"
    );
    println!(
        "[summary] anthropic text_chars={} reasoning_chars={}",
        codex_live_tests::text_len(&events),
        codex_live_tests::reasoning_len(&events)
    );
    codex_live_tests::assert_completed_with_usage(&events, &ctx);
    codex_live_tests::assert_reasoning_before_message(&events, &ctx);
}

// ================================================================
// Matrix generation
// ================================================================

/// Generates one test per (vendor, scenario) for each bridge. Vendors listed
/// here skip at runtime when unconfigured — keep this list in sync with
/// `LIVE_VENDORS` in `.env.local`.
macro_rules! bridge_matrix {
    ($suffix:ident, $scenario:ident, [$($vendor:literal),*]) => {
        paste::paste! {
            $(
                #[tokio::test]
                async fn [<$vendor _genai_ $suffix>]() {
                    match codex_live_tests::vendor($vendor) {
                        Some(cfg) => $scenario(&cfg, Bridge::Genai).await,
                        None => println!("vendor `{}` not configured — skipping", $vendor),
                    }
                }
                #[tokio::test]
                async fn [<$vendor _rig_ $suffix>]() {
                    match codex_live_tests::vendor($vendor) {
                        Some(cfg) => $scenario(&cfg, Bridge::Rig).await,
                        None => println!("vendor `{}` not configured — skipping", $vendor),
                    }
                }
            )*
        }
    };
}

bridge_matrix!(chat, scenario_chat, ["mimo", "glm"]);
bridge_matrix!(effort_low, scenario_effort_low, ["mimo", "glm"]);
bridge_matrix!(tool_round_trip, scenario_tool_round_trip, ["mimo", "glm"]);
bridge_matrix!(anthropic, scenario_anthropic, ["mimo", "glm"]);

// ================================================================
// Helpers
// ================================================================

fn base_request(cfg: &LiveConfig, instructions: &str, prompt: &str) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: cfg.model.clone(),
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

fn extract_function_call(events: &[ResponseEvent]) -> Option<(String, String, String)> {
    events.iter().find_map(|e| match e {
        ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        }) => Some((name.clone(), arguments.clone(), call_id.clone())),
        _ => None,
    })
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
