//! Bridge-level live matrix: scenarios run through the rig bridge (the fork
//! default) against EVERY configured vendor (`LIVE_VENDORS`, default mimo),
//! asserting codex's protocol invariants on real model streams. Tests are
//! generated per vendor × bridge so nextest reports each combination
//! separately; vendors without an Anthropic gateway skip that scenario.
//!
//! The genai bridge is shelved: its variants only run when
//! `LIVE_INCLUDE_GENAI=1` is set (re-validation / A/B comparison runs).
//!
//! Adding a vendor: add it to the `matrix!` list below + set its
//! `LIVE_<NAME>_*` variables in `.env.local` — nothing else.

// Scenario helpers sit outside `#[test]` functions, so Clippy.toml's
// `allow-expect-in-tests` does not reach them syntactically; panicking on a
// violated scenario invariant is the intended fail-fast behavior here.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

use codex_api::ResponsesApiRequest;
use codex_api::ResponseEvent;
use codex_live_tests::anthropic_url_or_skip;
use codex_live_tests::run_turn;
use codex_live_tests::user_message;
use codex_live_tests::Bridge;
use codex_live_tests::LiveWire;
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
    let events = run_turn(cfg, &cfg.base_url, LiveWire::Chat, bridge, &request, "chat").await;

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
    let events = run_turn(cfg, &cfg.base_url, LiveWire::Chat, bridge, &request, "effort").await;
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

    let turn1 = run_turn(cfg, &cfg.base_url, LiveWire::Chat, bridge, &request, "tool-t1").await;
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

    let turn2 = run_turn(cfg, &cfg.base_url, LiveWire::Chat, bridge, &request, "tool-t2").await;
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
    let events = run_turn(cfg, &anthropic_url, LiveWire::Anthropic, bridge, &request, "anthropic").await;

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

/// Error path: an invalid key must surface as an HTTP 401 transport error
/// — the rig bridge preserves the status so codex-core's re-login loop can
/// trigger. The genai bridge historically flattens errors to a network
/// string, so only the string content is asserted there.
async fn scenario_auth_rejected(cfg: &LiveConfig, bridge: Bridge) {
    // Auth testing is inherently live: it sends an invalid key to a real
    // endpoint to verify the 401 mapping. No fixture can represent this.
    if std::env::var("LIVE_CASSETTE").as_deref() == Ok("replay") {
        println!("{}/{} auth: live-only scenario, skipping in replay", cfg.vendor, bridge.name());
        return;
    }
    use codex_live_tests::turn_start_error;
    let request = base_request(cfg, "You are a helpful assistant.", "hi");
    let error = turn_start_error(cfg, &cfg.base_url, bridge, &request)
        .await
        .unwrap_or_else(|| panic!("{}/{} auth: expected a start error for an invalid key", cfg.vendor, bridge.name()));
    println!("[summary] {}/{} auth error: {error}", cfg.vendor, bridge.name());
    assert!(
        error.contains("401"),
        "{}/{} auth: error should mention 401, got: {error}",
        cfg.vendor,
        bridge.name()
    );
    if bridge == Bridge::Rig {
        // Strict for rig: the status code survives mapping (Http{401}).
        assert!(
            error.to_lowercase().contains("http"),
            "{}/{} auth: rig errors should surface as HTTP transport errors, got: {error}",
            cfg.vendor,
            bridge.name()
        );
    }
}

/// Anthropic wire with a tool round trip — the classic breakage point is
/// thinking-block + signature replay on the second turn.
async fn scenario_anthropic_tool_round_trip(cfg: &LiveConfig, bridge: Bridge) {
    let Some(anthropic_url) = anthropic_url_or_skip(cfg) else {
        return;
    };
    let ctx = format!("{}/{} anthropic-tool", cfg.vendor, bridge.name());
    let mut request = base_request(
        cfg,
        "You are a helpful assistant.",
        "北京今天天气怎么样？请务必调用 get_weather 工具查询，不要凭空回答。",
    );
    request.tools = Some(weather_tools());
    request.parallel_tool_calls = false;

    let turn1 = run_turn(cfg, &anthropic_url, LiveWire::Anthropic, bridge, &request, "anthropic-tool-t1").await;
    codex_live_tests::assert_completed_with_usage(&turn1, &ctx);
    assert_eq!(
        codex_live_tests::end_turn_of(&turn1),
        Some(false),
        "{ctx}: tool turn should end with end_turn=false"
    );
    let function_call =
        extract_function_call(&turn1).expect("anthropic tool call emitted");
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

    let turn2 = run_turn(cfg, &anthropic_url, LiveWire::Anthropic, bridge, &request, "anthropic-tool-t2").await;
    assert!(
        codex_live_tests::text_len(&turn2) > 0,
        "{ctx}: expected a final answer after the tool result"
    );
    codex_live_tests::assert_completed_with_usage(&turn2, &ctx);
}

/// Parallel tool calls: the prompt demands two independent calls; the
/// multi-call accumulator is exercised whenever the model complies (the
/// count is reported — strictly-parallel behavior varies by model).
async fn scenario_parallel_tools(cfg: &LiveConfig, bridge: Bridge) {
    let ctx = format!("{}/{} parallel-tools", cfg.vendor, bridge.name());
    let mut request = base_request(
        cfg,
        "You are a helpful assistant.",
        "请分别查询北京和上海两个城市的天气：用两次独立的 get_weather 工具调用（一次查北京，一次查上海，不要合并成一次调用），然后一起告诉我。",
    );
    request.tools = Some(weather_tools());
    request.parallel_tool_calls = true;

    let events = run_turn(cfg, &cfg.base_url, LiveWire::Chat, bridge, &request, "parallel-tools").await;
    codex_live_tests::assert_completed_with_usage(&events, &ctx);
    let calls = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { .. })
            )
        })
        .count();
    println!("[summary] {ctx}: {calls} tool call(s) in one turn");
    assert!(calls >= 1, "{ctx}: expected at least one tool call");
}

// ================================================================
// Matrix generation
// ================================================================

/// Generates one test per (vendor, scenario) for each bridge. The genai
/// variants are gated behind `LIVE_INCLUDE_GENAI=1` (bridge shelved; rig is
/// the fork default). Vendors listed here skip at runtime when unconfigured
/// — keep this list in sync with `LIVE_VENDORS` in `.env.local`.
macro_rules! bridge_matrix {
    ($suffix:ident, $scenario:ident, [$($vendor:literal),*]) => {
        paste::paste! {
            $(
                #[tokio::test]
                async fn [<$vendor _genai_ $suffix>]() {
                    if !codex_live_tests::genai_bridge_enabled() {
                        println!("genai bridge shelved (rig is the fork default) — set LIVE_INCLUDE_GENAI=1 to include");
                        return;
                    }
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

bridge_matrix!(chat, scenario_chat, ["mimo", "glm", "step"]);
bridge_matrix!(effort_low, scenario_effort_low, ["mimo", "glm", "step"]);
bridge_matrix!(tool_round_trip, scenario_tool_round_trip, ["mimo", "glm", "step"]);
bridge_matrix!(anthropic, scenario_anthropic, ["mimo", "glm", "step"]);
bridge_matrix!(anthropic_tool_round_trip, scenario_anthropic_tool_round_trip, ["mimo", "glm", "step"]);
bridge_matrix!(parallel_tools, scenario_parallel_tools, ["mimo", "glm", "step"]);
bridge_matrix!(auth_rejected, scenario_auth_rejected, ["mimo", "glm", "step"]);

/// Scenario families where the MODEL legitimately varies per call whether
/// it emits thinking or a text preamble before/around tool calls. Two live
/// recordings (one per bridge) of these can differ in collapsed kind
/// sequences while the bridges remain structurally equivalent — structural
/// parity for them is enforced by the scenario invariants themselves
/// (ordering, delta reassembly, end_turn, usage).
///
/// The strict scenarios (chat, effort, anthropic, auth) stay fully diffed —
/// the two real bridge bugs this diff caught (missing `Created` event,
/// OpenAI-only params flattened onto the Anthropic wire) surfaced there.
const MODEL_NONDETERMINISTIC_TAG_FAMILIES: &[&str] = &[
    "tool-t1",
    "tool-t2",
    "parallel-tools",
    "anthropic-tool-t1",
    "anthropic-tool-t2",
];

fn is_model_nondeterministic(tag: &str) -> bool {
    MODEL_NONDETERMINISTIC_TAG_FAMILIES.contains(&tag)
}

/// A/B diff (offline): when cassette fixtures exist for both bridges of the
/// same vendor+tag, their event-kind sequences must match — an automatic
/// structural equivalence check between the genai and rig bridges. Inactive
/// while genai is shelved (no `LIVE_INCLUDE_GENAI=1`): rig is allowed to
/// evolve past genai's shape.
#[test]
fn ab_diff_fixtures() {
    if !codex_live_tests::genai_bridge_enabled() {
        println!("genai shelved — A/B bridge diff inactive (set LIVE_INCLUDE_GENAI=1 to enable)");
        return;
    }
    let vendors = codex_live_tests::vendors();
    if vendors.is_empty() {
        println!("no vendors configured — skipping");
        return;
    }
    let mut checked = 0;
    for cfg in &vendors {
        let Ok(entries) = std::fs::read_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(&cfg.vendor),
        ) else {
            continue;
        };
        let mut tags: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.strip_prefix("genai-")?.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        tags.sort();
        for tag in tags {
            let (Some(g), Some(r)) = (
                codex_live_tests::load_fixture(&cfg.vendor, "genai", &tag),
                codex_live_tests::load_fixture(&cfg.vendor, "rig", &tag),
            ) else {
                continue;
            };
            // Compare COLLAPSED kind sequences: consecutive duplicate kinds
            // (delta granularity) are provider-stream internals, not a
            // semantic difference between the bridges.
            fn collapse(kinds: Vec<&str>) -> Vec<&str> {
                let mut out: Vec<&str> = Vec::new();
                for kind in kinds {
                    if out.last() != Some(&kind) {
                        out.push(kind);
                    }
                }
                out
            }
            let gk = collapse(g.events.iter().map(codex_live_tests::event_kind).collect());
            let rk = collapse(r.events.iter().map(codex_live_tests::event_kind).collect());
            if gk != rk && is_model_nondeterministic(&tag) {
                println!(
                    "[ab-diff] {}/{}: model-nondeterministic family, structural                      invariants enforced by the scenario itself — skipping kind diff",
                    cfg.vendor, tag
                );
                continue;
            }
            assert_eq!(
                gk, rk,
                "{}/{}: collapsed event-kind sequences differ between genai and rig fixtures",
                cfg.vendor, tag
            );
            checked += 1;
        }
    }
    println!("[summary] ab_diff checked {checked} fixture pair(s)");
}

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
