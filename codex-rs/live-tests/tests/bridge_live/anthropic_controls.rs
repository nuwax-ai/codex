use super::*;

/// R02 live gate: the Anthropic wire now carries the output schema through
/// additional_params.output_config. This checks gateway acceptance, not
/// output schema compliance or preservation of thinking behavior.
pub(super) async fn scenario_anthropic_output_schema(cfg: &LiveConfig, bridge: Bridge) {
    let Some(anthropic_url) = anthropic_url_or_skip(cfg) else {
        return;
    };
    let mut request = base_request(
        cfg,
        "You are a helpful assistant. Answer in Chinese.",
        "北京和上海分别叫什么名字?",
    );
    request.text = Some(codex_api::TextControls {
        verbosity: None,
        format: Some(codex_api::TextFormat {
            r#type: codex_api::TextFormatType::JsonSchema,
            strict: false,
            name: "city_pair".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "city1": {"type": "string"},
                    "city2": {"type": "string"}
                },
                "required": ["city1", "city2"],
                "additionalProperties": false
            }),
        }),
    });
    let events = run_turn(
        cfg,
        &anthropic_url,
        LiveWire::Anthropic,
        bridge,
        &request,
        "anthropic-schema",
    )
    .await;

    let ctx = format!("{}/{} anthropic-schema", cfg.vendor, bridge.name());
    assert!(
        codex_live_tests::text_len(&events) > 0,
        "{ctx}: expected text output; a gateway rejecting output_config would fail here"
    );
    codex_live_tests::assert_completed_with_usage(&events, &ctx);
    println!(
        "[summary] {ctx}: reasoning_chars={} (diagnostic only; schema compliance is not checked)",
        codex_live_tests::reasoning_len(&events)
    );
}

/// R10 live gate: explicit effort none injects thinking:{type:"disabled"}.
/// A gateway that rejects the field must surface an error (fail-fast), and
/// visible reasoning is recorded separately from gateway acceptance.
pub(super) async fn scenario_anthropic_effort_none(cfg: &LiveConfig, bridge: Bridge) {
    let Some(anthropic_url) = anthropic_url_or_skip(cfg) else {
        return;
    };
    let mut request = base_request(
        cfg,
        "You are a helpful assistant. Answer in Chinese.",
        "用一句话回答:法国的首都是哪里?",
    );
    request.reasoning = Some(codex_api::Reasoning {
        effort: Some(codex_protocol::openai_models::ReasoningEffort::None),
        summary: None,
        context: None,
    });
    let events = run_turn(
        cfg,
        &anthropic_url,
        LiveWire::Anthropic,
        bridge,
        &request,
        "anthropic-effort-none",
    )
    .await;

    let ctx = format!("{}/{} anthropic-effort-none", cfg.vendor, bridge.name());
    assert!(
        codex_live_tests::text_len(&events) > 0,
        "{ctx}: expected an answer with thinking disabled"
    );
    // Acceptance is the hard gate (a rejecting gateway fails the turn).
    // Honoring is diagnostic: gateways may silently ignore the disabled
    // config (observed: StepFun still emits thinking). Zero visible reasoning
    // does not prove the absence of hidden computation.
    let reasoning = codex_live_tests::reasoning_len(&events);
    if reasoning > 0 {
        println!(
            "[summary] {ctx}: visible reasoning despite requested thinking:{{disabled}} ({reasoning} chars)"
        );
    }
    codex_live_tests::assert_completed_with_usage(&events, &ctx);
}

/// R09 acceptance gate: submit a failed tool result and require a completed
/// follow-up with text. HTTP mock tests check the actual is_error mapping.
pub(super) async fn scenario_anthropic_tool_error(cfg: &LiveConfig, bridge: Bridge) {
    let Some(anthropic_url) = anthropic_url_or_skip(cfg) else {
        return;
    };
    scenario_anthropic_tool_error_with_runner(cfg, bridge, |request, tag| {
        let url = &anthropic_url;
        async move { run_turn(cfg, url, LiveWire::Anthropic, bridge, &request, tag).await }
    })
    .await;
}

async fn scenario_anthropic_tool_error_with_runner<F, Fut>(
    cfg: &LiveConfig,
    bridge: Bridge,
    mut run: F,
) where
    F: FnMut(ResponsesApiRequest, &'static str) -> Fut,
    Fut: std::future::Future<Output = Vec<ResponseEvent>>,
{
    let ctx = format!("{}/{} anthropic-tool-error", cfg.vendor, bridge.name());
    let mut request = base_request(
        cfg,
        "You are a helpful assistant.",
        "请调用 get_weather 查询北京天气;如果工具报错,请直接告诉用户工具失败了。",
    );
    request.tools = Some(weather_tools());
    request.parallel_tool_calls = false;

    let turn1 = run(request.clone(), "anthropic-tool-err-t1").await;
    codex_live_tests::assert_completed_with_usage(&turn1, &ctx);
    let function_call =
        extract_function_call(&turn1).unwrap_or_else(|| panic!("{ctx}: tool call emitted"));
    assert_eq!(function_call.0, "get_weather");

    let mut input = request.input.clone();
    let (_, _, call_id) = function_call;
    append_turn_outputs(&mut input, &turn1);
    input.push(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                r#"{"error":"weather service unavailable (simulated)"}"#.into(),
            ),
            success: Some(false),
        },
        internal_chat_message_metadata_passthrough: None,
    });
    request.input = input;

    let turn2 = run(request, "anthropic-tool-err-t2").await;
    assert!(
        codex_live_tests::text_len(&turn2) > 0,
        "{ctx}: expected follow-up text after the failed tool result"
    );
    codex_live_tests::assert_completed_with_usage(&turn2, &ctx);
}

#[cfg(test)]
#[path = "anthropic_controls_tests.rs"]
mod tests;
