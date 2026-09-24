//! Live integration tests for the rig bridge, run against the Xiaomi MiMo
//! endpoints — the rig counterpart of
//! `codex-rust-genai-bridge/tests/live_mimo.rs` (same invariants, same
//! configuration, different bridge), so the two can be A/B compared.
//!
//! # Running
//!
//! ```text
//! cargo nextest run -p codex-rust-rig-bridge --no-capture
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponsesApiRequest;
use codex_api::ResponseStream;
use codex_api::RetryConfig;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
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
            println!("MIMO_API_KEY not set — skipping live MiMo rig test");
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
            let mut value = value.trim().to_string();
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = value[1..value.len() - 1].to_string();
            }
            merged.insert(key.trim().to_string(), value);
        }
    }
    merged
}

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

async fn run_turn(
    request: &ResponsesApiRequest,
    base_url: &str,
    api_key: &str,
) -> Vec<ResponseEvent> {
    let provider = mimo_provider(base_url);
    let auth: SharedAuthProvider = Arc::new(StaticBearerAuth(api_key.to_string()));

    let mut stream: ResponseStream = timeout(
        TURN_TIMEOUT,
        codex_rust_rig_bridge::stream_via_rig(
            request,
            &provider,
            &auth,
            HeaderMap::new(),
            provider.stream_idle_timeout,
        ),
    )
    .await
    .expect("stream_via_rig started within timeout")
    .expect("stream_via_rig succeeded");

    let mut events = Vec::new();
    loop {
        let event = timeout(TURN_TIMEOUT, stream.rx_event.recv())
            .await
            .expect("next event within timeout");
        match event {
            Some(Ok(event)) => {
                println!("[mimo-rig] {event:?}");
                events.push(event);
            }
            Some(Err(err)) => panic!("stream error from rig bridge: {err:#}"),
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
    assert!(
        events.iter().any(|e| matches!(
            e,
            ResponseEvent::Completed {
                token_usage: Some(_),
                ..
            }
        )),
        "{context}: Completed event should carry token usage"
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

fn assert_reasoning_before_message(events: &[ResponseEvent], context: &str) {
    let position_of =
        |pred: &dyn Fn(&ResponseEvent) -> bool| events.iter().position(|e| pred(e));
    if let (Some(r), Some(t)) = (
        position_of(&|e| matches!(e, ResponseEvent::ReasoningContentDelta { .. })),
        position_of(&|e| matches!(e, ResponseEvent::OutputTextDelta(_))),
    ) {
        assert!(r < t, "{context}: reasoning must start before text ({r} vs {t})");
    }
    if let (Some(r), Some(m)) = (
        position_of(&|e| {
            matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }))
        }),
        position_of(&|e| {
            matches!(e, ResponseEvent::OutputItemDone(ResponseItem::Message { .. }))
        }),
    ) {
        assert!(r < m, "{context}: reasoning item before message item ({r} vs {m})");
    }
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

    let events = run_turn(&request, &cfg.base_url, &cfg.api_key).await;

    assert!(text_len(&events) > 0, "expected at least one OutputTextDelta");
    println!(
        "[summary] text_chars={} reasoning_chars={}",
        text_len(&events),
        reasoning_len(&events)
    );
    assert_completed_with_usage(&events, "rig chat round trip");
    assert_reasoning_before_message(&events, "rig chat round trip");
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

    let turn1 = run_turn(&request, &cfg.base_url, &cfg.api_key).await;
    assert_completed_with_usage(&turn1, "rig tool turn 1");
    assert_reasoning_before_message(&turn1, "rig tool turn 1");
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

    // Turn 2: replay history plus the tool result — the stateless multi-turn
    // shape codex uses for chat providers.
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
    let turn2 = run_turn(&turn2_request, &cfg.base_url, &cfg.api_key).await;
    assert!(
        text_len(&turn2) > 0,
        "expected the model to answer with text after the tool result"
    );
    println!("[summary] final_answer_chars={}", text_len(&turn2));
    assert_completed_with_usage(&turn2, "rig tool turn 2");
}

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

    let events = run_turn(&request, &cfg.anthropic_base_url, &cfg.api_key).await;

    assert!(
        text_len(&events) > 0,
        "expected text output through the Anthropic protocol"
    );
    println!(
        "[summary] anthropic text_chars={} reasoning_chars={}",
        text_len(&events),
        reasoning_len(&events)
    );
    assert_completed_with_usage(&events, "rig anthropic chat round trip");
    assert_reasoning_before_message(&events, "rig anthropic chat round trip");
}
