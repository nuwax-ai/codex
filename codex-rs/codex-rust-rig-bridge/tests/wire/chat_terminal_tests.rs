use super::error_tests::provider;
use super::support;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn chat_chunk(choice: Value) -> Value {
    json!({
        "id": "chatcmpl-terminal", "object": "chat.completion.chunk", "created": 1,
        "model": "review-model", "choices": [choice],
    })
}

fn usage_chunk() -> Value {
    json!({
        "id": "chatcmpl-terminal", "object": "chat.completion.chunk", "created": 1,
        "model": "review-model", "choices": [],
        "usage": {"prompt_tokens": 23, "completion_tokens": 7, "total_tokens": 30},
    })
}

async fn run_chat(frames: Vec<Value>) -> Vec<Result<ResponseEvent, ApiError>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut payload: String = frames
            .into_iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect();
        payload.push_str("data: [DONE]\n\n");
        support::serve_payload(&listener, &payload).await
    });
    let mut request = support::request(vec![support::user()]);
    request.max_output_tokens = Some(64);
    support::set_tools(
        &mut request,
        json!([{"type":"function","name":"lookup","parameters":{"type":"object","properties":{}}}]),
    );
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Chat,
        Duration::from_secs(3),
    )
    .await
    .expect("start real Chat HTTP stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    let wire = server.await.expect("HTTP server task");
    assert_eq!(wire["body"]["max_tokens"], json!(64));
    events
}

fn assert_cap_failure(events: &[Result<ResponseEvent, ApiError>]) {
    let errors: Vec<_> = events
        .iter()
        .filter_map(|event| event.as_ref().err())
        .collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert!(matches!(
        errors[0],
        ApiError::CapExhausted { message, .. } if message.contains("Output token limit reached")
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. })))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::OutputItemDone(_)))),
        "a cap must not promote pending content to completed items: {events:?}"
    );
}

fn assert_visible_output(
    events: &[Result<ResponseEvent, ApiError>],
    expected_text: &str,
    expected_usage: Vec<TokenUsage>,
) {
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputTextDelta(text)) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let usage: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::Completed { token_usage, .. }) => token_usage.clone(),
            _ => None,
        })
        .collect();
    assert_eq!((text, usage), (expected_text.to_owned(), expected_usage));
    assert!(
        !events.iter().any(|event| matches!(
            event,
            Ok(ResponseEvent::OutputItemAdded(
                ResponseItem::FunctionCall { .. } | ResponseItem::CustomToolCall { .. }
            ) | ResponseEvent::OutputItemDone(
                ResponseItem::FunctionCall { .. } | ResponseItem::CustomToolCall { .. }
            ) | ResponseEvent::ToolCallInputDelta { .. })
        )),
        "late tool content must not enter the response: {events:?}"
    );
}

fn expected_usage() -> Vec<TokenUsage> {
    vec![TokenUsage {
        input_tokens: 23,
        output_tokens: 7,
        total_tokens: 30,
        ..Default::default()
    }]
}

#[tokio::test]
async fn chat_reasoning_can_finish_after_the_answer_text() {
    let events = run_chat(vec![
        chat_chunk(
            json!({"index":0,"delta":{"reasoning_content":"thinking"},"finish_reason":null}),
        ),
        chat_chunk(json!({"index":0,"delta":{"content":"answer"},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"stop"})),
        usage_chunk(),
    ])
    .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_visible_output(&events, "answer", expected_usage());
    let reasoning: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                content,
                encrypted_content,
                ..
            })) => Some((content.clone(), encrypted_content.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning.len(), 1);
    assert_eq!(
        reasoning[0].0,
        Some(vec![
            codex_protocol::models::ReasoningItemContent::ReasoningText {
                text: "thinking".to_string(),
            }
        ])
    );
    let envelope = reasoning[0]
        .1
        .as_deref()
        .expect("reasoning replay envelope");
    let envelope: Value =
        serde_json::from_str(envelope.strip_prefix("codex-rig-reasoning-v2:").unwrap()).unwrap();
    assert_eq!(
        envelope["blocks"][0]["native"]["item"]["reasoning_content"],
        "thinking"
    );
}

#[tokio::test]
async fn chat_reasoning_can_finish_after_a_tool_call() {
    let events = run_chat(vec![
        chat_chunk(json!({"index":0,"delta":{"reasoning_content":"thinking"},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":"call-after-reasoning","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"tool_calls"})),
        usage_chunk(),
    ])
    .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })) => Some("reasoning"),
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, .. })) => {
                assert_eq!(call_id, "call-after-reasoning");
                Some("tool")
            }
            Ok(ResponseEvent::Completed {
                end_turn,
                token_usage,
                ..
            }) => {
                assert_eq!(end_turn, &Some(false));
                assert_eq!(token_usage.as_ref(), expected_usage().first());
                Some("completed")
            }
            _ => None,
        })
        .collect();
    assert_eq!(completed, ["reasoning", "tool", "completed"]);
}

#[tokio::test]
async fn empty_finish_reason_does_not_drop_the_real_length_terminal() {
    let events = run_chat(vec![
        chat_chunk(json!({"index":0,"delta":{"content":"kept"},"finish_reason":""})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"length"})),
    ])
    .await;
    assert_cap_failure(&events);
    assert_visible_output(&events, "kept", Vec::new());
}

#[tokio::test]
async fn secondary_choice_terminal_does_not_freeze_the_primary_choice() {
    let events = run_chat(vec![
        chat_chunk(json!({"index":0,"delta":{"content":"kept"},"finish_reason":null})),
        chat_chunk(json!({"index":1,"delta":{"content":"other candidate"},"finish_reason":"stop"})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"length"})),
    ])
    .await;
    assert_cap_failure(&events);
    assert_visible_output(&events, "kept", Vec::new());
}

#[tokio::test]
async fn primary_choice_without_index_preserves_stop_and_trailing_usage() {
    let events = run_chat(vec![
        chat_chunk(json!({"delta":{"content":"kept"},"finish_reason":null})),
        chat_chunk(json!({"delta":{},"finish_reason":"stop"})),
        usage_chunk(),
    ])
    .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_visible_output(&events, "kept", expected_usage());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                Ok(ResponseEvent::Completed {
                    end_turn: Some(true),
                    ..
                })
            ))
            .count(),
        1,
    );
}

#[tokio::test]
async fn late_primary_text_and_tools_do_not_mutate_the_first_stop() {
    let events = run_chat(vec![
        chat_chunk(json!({"index":0,"delta":{"content":"kept"},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"stop"})),
        chat_chunk(json!({"index":0,"delta":{"content":"late text"},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":"late-call","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"length"})),
        usage_chunk(),
    ])
    .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_visible_output(&events, "kept", expected_usage());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                Ok(ResponseEvent::Completed {
                    end_turn: Some(true),
                    ..
                })
            ))
            .count(),
        1,
    );
}

#[tokio::test]
async fn late_primary_text_and_tools_do_not_escape_the_length_error() {
    let events = run_chat(vec![
        chat_chunk(json!({"index":0,"delta":{"content":"kept"},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"length"})),
        chat_chunk(json!({"index":0,"delta":{"content":"late text"},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":"late-call","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":null})),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"stop"})),
        usage_chunk(),
    ])
    .await;
    assert_cap_failure(&events);
    assert_visible_output(&events, "kept", Vec::new());
}

/// Rig closes a tool call the output budget cut mid-arguments without its
/// native item. That close must not fail the stream with a tool-shape
/// complaint — the cap terminal explains the cut, and the truncated call
/// never publishes executable items.
#[tokio::test]
async fn a_budget_cut_tool_call_does_not_replace_the_cap_terminal() {
    let events = run_chat(vec![
        chat_chunk(
            json!({"index":0,"delta":{"role":"assistant","content":"partial cap answer"},"finish_reason":null}),
        ),
        chat_chunk(
            json!({"index":0,"delta":{"tool_calls":[{
                "index":0,"id":"call_cut","type":"function",
                "function":{"name":"lookup","arguments":"{\"q\":\"ab"}
            }]},"finish_reason":null}),
        ),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"length"})),
        usage_chunk(),
    ])
    .await;
    assert_cap_failure(&events);
    assert_visible_output(&events, "partial cap answer", Vec::new());
}

/// A present usage object with malformed counters is a wire failure core can
/// retry, not a silent default terminal: rig's lenient reader would fold it
/// into `Usage::default`, so the bridge's wire check rejects the frame.
#[tokio::test]
async fn malformed_usage_counters_fail_the_stream_instead_of_folding_a_terminal() {
    let events = run_chat(vec![
        chat_chunk(
            json!({"index":0,"delta":{"role":"assistant","content":"kept"},"finish_reason":null}),
        ),
        chat_chunk(json!({"index":0,"delta":{},"finish_reason":"stop"})),
        json!({
            "id": "chatcmpl-terminal", "object": "chat.completion.chunk", "created": 1,
            "model": "review-model", "choices": [],
            "usage": {"prompt_tokens": -5, "completion_tokens": 7, "total_tokens": 7},
        }),
    ])
    .await;
    let errors: Vec<_> = events
        .iter()
        .filter_map(|event| event.as_ref().err())
        .collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    match &errors[0] {
        codex_api::ApiError::Transport(codex_api::TransportError::Network(message)) => {
            assert!(
                message.contains("invalid value: integer `-5`"),
                "the wire counter error survives the provider error mapping: {message}"
            );
        }
        other => {
            panic!("malformed usage must surface as a retryable transport error, got {other:?}")
        }
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. })))
    );
}
