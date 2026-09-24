//! Live integration tests for the Chat Completions (`wire_api = "chat"`)
//! bridge, run against the Xiaomi MiMo OpenAI-compatible endpoint.
//!
//! These tests exercise the real network path that `codex-core` uses:
//! `ResponsesApiRequest` → genai `ChatRequest` → MiMo `/chat/completions`
//! (SSE) → codex `ResponseEvent`s. Every event is printed so model output is
//! captured in the test log for manual inspection.
//!
//! # Configuration
//!
//! Credentials are read from environment variables, falling back to a
//! `.env.local` / `.env` file at the repository root (both are gitignored —
//! never commit real keys):
//!
//! ```text
//! MIMO_API_KEY=...
//! MIMO_BASE_URL=https://token-plan-cn.xiaomimimo.com/v1
//! MIMO_MODEL=mimo-v2.6-flash
//! ```
//!
//! When `MIMO_API_KEY` is not configured the tests are skipped (they pass
//! with a notice), so environments without secrets stay green.
//!
//! # Running
//!
//! Print the model output with:
//!
//! ```text
//! cargo nextest run -p codex-rust-genai-bridge --no-capture live_mimo
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::Reasoning;
use codex_api::ResponseEvent;
use codex_api::ResponsesApiRequest;
use codex_api::ResponseStream;
use codex_api::RetryConfig;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use genai::adapter::AdapterKind;
use http::HeaderMap;
use http::header::AUTHORIZATION;
use tokio::time::timeout;

const TURN_TIMEOUT: Duration = Duration::from_secs(180);

struct LiveConfig {
    api_key: String,
    base_url: String,
    anthropic_base_url: String,
    model: String,
}

/// Loads `MIMO_*` configuration: real environment variables first, then
/// `.env.local` / `.env` at the repo root as fallback. Files are parsed into
/// a local map — nothing is written to the process environment, which would
/// require `unsafe` on edition 2024.
fn live_config() -> Option<LiveConfig> {
    let file_env = load_env_files();
    let lookup = |key: &str| -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| file_env.get(key).cloned())
    };
    let api_key = match lookup("MIMO_API_KEY") {
        Some(key) => key,
        None => {
            println!("MIMO_API_KEY not set — skipping live MiMo test");
            return None;
        }
    };
    Some(LiveConfig {
        api_key,
        base_url: lookup("MIMO_BASE_URL")
            .unwrap_or_else(|| "https://token-plan-cn.xiaomimimo.com/v1".into()),
        anthropic_base_url: lookup("MIMO_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|| "https://token-plan-cn.xiaomimimo.com/anthropic/v1".into()),
        model: lookup("MIMO_MODEL").unwrap_or_else(|| "mimo-v2.6-flash".into()),
    })
}

/// Parses `.env.local` (higher priority) then `.env`, walking up from the
/// crate manifest to the repository root.
fn load_env_files() -> HashMap<String, String> {
    let mut merged = HashMap::new();
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut repo_root = None;
    while dir.pop() {
        if dir.join(".git").exists() {
            repo_root = Some(dir);
            break;
        }
    }
    let Some(root) = repo_root else {
        return merged;
    };
    for file in [".env.local", ".env"] {
        let Ok(contents) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let mut value = value.trim().to_string();
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = value[1..value.len() - 1].to_string();
            }
            merged.insert(key.to_string(), value);
        }
    }
    merged
}

/// Bearer-token [`AuthProvider`] backed by a static key from `.env.local`.
struct StaticBearerAuth(String);

impl AuthProvider for StaticBearerAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        let value = format!("Bearer {}", self.0)
            .parse()
            .expect("valid authorization header value");
        headers.insert(AUTHORIZATION, value);
    }
}

fn mimo_provider(base_url: &str) -> Provider {
    Provider {
        name: "mimo".into(),
        base_url: base_url.to_string(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(120),
    }
}

fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

/// Streams one turn through the bridge, printing every `ResponseEvent`.
/// `adapter_kind` selects the wire protocol genai speaks (OpenAI Chat
/// Completions vs Anthropic Messages); the conversion layer is identical.
async fn run_turn(
    request: &ResponsesApiRequest,
    base_url: &str,
    api_key: &str,
    adapter_kind: AdapterKind,
) -> Vec<ResponseEvent> {
    let provider = mimo_provider(base_url);
    let auth: SharedAuthProvider = Arc::new(StaticBearerAuth(api_key.to_string()));

    let mut stream: ResponseStream = timeout(
        TURN_TIMEOUT,
        codex_rust_genai_bridge::stream_via_genai(
            request,
            &provider,
            &auth,
            HeaderMap::new(),
            adapter_kind,
            provider.stream_idle_timeout,
        ),
    )
    .await
    .expect("stream_via_genai started within timeout")
    .expect("stream_via_genai succeeded");

    let mut events = Vec::new();
    loop {
        let event = timeout(TURN_TIMEOUT, stream.rx_event.recv())
            .await
            .expect("next event within timeout");
        match event {
            Some(Ok(event)) => {
                println!("[mimo] {event:?}");
                events.push(event);
            }
            Some(Err(err)) => panic!("stream error from bridge: {err:#}"),
            None => break,
        }
    }
    events
}

fn assert_completed_with_usage(events: &[ResponseEvent], context: &str) {
    let completed = events
        .iter()
        .filter(|e| matches!(e, ResponseEvent::Completed { .. }))
        .count();
    assert_eq!(completed, 1, "{context}: expected exactly one Completed event");
    let has_usage = events.iter().any(|e| {
        matches!(
            e,
            ResponseEvent::Completed {
                token_usage: Some(_),
                ..
            }
        )
    });
    assert!(
        has_usage,
        "{context}: Completed event should carry token usage"
    );
}

/// Position of the first event matching the predicate, for ordering checks.
fn position_of(events: &[ResponseEvent], mut pred: impl FnMut(&ResponseEvent) -> bool) -> Option<usize> {
    events.iter().position(|e| pred(e))
}

/// Asserts the v0.17.4 event-ordering fix holds on the wire: reasoning
/// starts streaming before message text, and the reasoning item completes
/// before the message item.
fn assert_reasoning_before_message(events: &[ResponseEvent], context: &str) {
    let first_reasoning = position_of(events, |e| {
        matches!(e, ResponseEvent::ReasoningContentDelta { .. })
    });
    let first_text = position_of(events, |e| matches!(e, ResponseEvent::OutputTextDelta(_)));
    if let (Some(r), Some(t)) = (first_reasoning, first_text) {
        assert!(
            r < t,
            "{context}: reasoning deltas must start before text deltas (got {r} vs {t})"
        );
    }
    let reasoning_done = position_of(events, |e| {
        matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }))
    });
    let message_done = position_of(events, |e| {
        matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Message { .. }))
    });
    if let (Some(r), Some(m)) = (reasoning_done, message_done) {
        assert!(
            r < m,
            "{context}: reasoning item must complete before the message item (got {r} vs {m})"
        );
    }
}

/// Concatenated `ToolCallInputDelta`s must reassemble into exactly the final
/// `FunctionCall.arguments` string — this validates the bridge's
/// prefix-diff delta computation on a real stream.
fn assert_tool_deltas_reassemble(events: &[ResponseEvent], context: &str) {
    let reassembled: String = events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let final_args = events
        .iter()
        .find_map(|e| match e {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }) => {
                Some(arguments.clone())
            }
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(
        reassembled, final_args,
        "{context}: concatenated tool-call deltas must equal the final arguments"
    );
}

fn text_len(events: &[ResponseEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            ResponseEvent::OutputTextDelta(delta) => delta.chars().count(),
            _ => 0,
        })
        .sum()
}

fn reasoning_len(events: &[ResponseEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            ResponseEvent::ReasoningContentDelta { delta, .. } => delta.chars().count(),
            _ => 0,
        })
        .sum()
}

#[tokio::test]
async fn chat_round_trip_streams_text_reasoning_and_usage() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = ResponsesApiRequest {
        model: cfg.model.clone(),
        instructions: "You are a helpful assistant. Answer in Chinese.".into(),
        input: vec![user_message("用一句话解释什么是斐波那契数列。")],
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
    };

    let events = run_turn(&request, &cfg.base_url, &cfg.api_key, AdapterKind::OpenAI).await;

    assert!(
        text_len(&events) > 0,
        "expected at least one OutputTextDelta"
    );
    let reasoning_chars = reasoning_len(&events);
    println!("[summary] text_chars={} reasoning_chars={reasoning_chars}", text_len(&events));
    assert_completed_with_usage(&events, "chat round trip");
    assert_reasoning_before_message(&events, "chat round trip");
    // A plain chat turn must affirmatively end the turn.
    let end_turn = events.iter().find_map(|e| match e {
        ResponseEvent::Completed { end_turn, .. } => *end_turn,
        _ => None,
    });
    assert_eq!(end_turn, Some(true), "plain chat turn should end with end_turn=true");
}

#[tokio::test]
async fn tool_call_round_trip_with_tool_result_replay() {
    let Some(cfg) = live_config() else {
        return;
    };

    // A flat function tool, the shape core registers for non-namespaced tools.
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
    let tools = codex_api::ResponsesApiTools::from(Arc::from(raw));

    let request = ResponsesApiRequest {
        model: cfg.model.clone(),
        instructions: "You are a helpful assistant.".into(),
        input: vec![user_message(
            "北京今天天气怎么样？请务必调用 get_weather 工具查询，不要凭空回答。",
        )],
        tools: Some(tools),
        tool_choice: "auto".into(),
        parallel_tool_calls: false,
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
    };

    // Turn 1: the model should emit a function call for get_weather.
    let turn1 =
        run_turn(&request, &cfg.base_url, &cfg.api_key, AdapterKind::OpenAI).await;
    assert_completed_with_usage(&turn1, "tool turn 1");
    assert_reasoning_before_message(&turn1, "tool turn 1");
    assert_tool_deltas_reassemble(&turn1, "tool turn 1");
    // A tool-call turn must NOT be treated as an affirmative end of turn.
    let end_turn_1 = turn1.iter().find_map(|e| match e {
        ResponseEvent::Completed { end_turn, .. } => *end_turn,
        _ => None,
    });
    assert_eq!(
        end_turn_1,
        Some(false),
        "tool-call turn should end with end_turn=false"
    );

    let function_call = turn1
        .iter()
        .find_map(|e| match e {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
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

    // Turn 2: replay the full history plus the tool result — this is the
    // stateless multi-turn shape codex uses for chat providers.
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

    let turn2_request = ResponsesApiRequest {
        input,
        ..request
    };
    let turn2 =
        run_turn(&turn2_request, &cfg.base_url, &cfg.api_key, AdapterKind::OpenAI).await;
    assert!(
        text_len(&turn2) > 0,
        "expected the model to answer with text after the tool result"
    );
    println!("[summary] final_answer_chars={}", text_len(&turn2));
    assert_completed_with_usage(&turn2, "tool turn 2");
}

#[tokio::test]
async fn chat_with_reasoning_effort_low_is_accepted() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = ResponsesApiRequest {
        model: cfg.model.clone(),
        instructions: "You are a helpful assistant.".into(),
        input: vec![user_message("1+1 等于几？直接回答。")],
        tools: None,
        tool_choice: "auto".into(),
        parallel_tool_calls: true,
        reasoning: Some(Reasoning {
            effort: Some(codex_protocol::openai_models::ReasoningEffort::Low),
            summary: None,
            context: None,
        }),
        store: false,
        stream: true,
        stream_options: None,
        include: vec![],
        service_tier: None,
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    };

    let events = run_turn(&request, &cfg.base_url, &cfg.api_key, AdapterKind::OpenAI).await;

    assert!(text_len(&events) > 0, "expected an answer to the question");
    assert_completed_with_usage(&events, "reasoning effort low");
}

/// Anthropic Messages protocol (`/anthropic` gateway): same bridge, genai's
/// Anthropic adapter. Proves the conversion layer is protocol-agnostic and
/// the adapter selection (see `adapter_kind_for_base_url` in codex-core)
/// covers Anthropic-protocol providers end to end.
#[tokio::test]
async fn anthropic_protocol_chat_round_trip() {
    let Some(cfg) = live_config() else {
        return;
    };
    let request = ResponsesApiRequest {
        model: cfg.model.clone(),
        instructions: "You are a helpful assistant. Answer in Chinese.".into(),
        input: vec![user_message("用一句话说明二分查找的思想。")],
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
    };

    let events = run_turn(
        &request,
        &cfg.anthropic_base_url,
        &cfg.api_key,
        AdapterKind::Anthropic,
    )
    .await;

    assert!(
        text_len(&events) > 0,
        "expected text output through the Anthropic protocol"
    );
    println!(
        "[summary] anthropic text_chars={} reasoning_chars={}",
        text_len(&events),
        reasoning_len(&events)
    );
    assert_completed_with_usage(&events, "anthropic chat round trip");
    assert_reasoning_before_message(&events, "anthropic chat round trip");
}
