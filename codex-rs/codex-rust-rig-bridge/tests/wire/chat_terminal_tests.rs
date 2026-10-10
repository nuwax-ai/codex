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
    // No synthesized Completed may follow a cap, but the flushed partial's
    // item Done (the transcript the model really streamed, made durable)
    // must PRECEDE the error — exactly one such Done.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. })))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Ok(ResponseEvent::OutputItemDone(_))))
            .count(),
        1,
        "exactly the flushed partial item may complete: {events:?}"
    );
    let done_index = events
        .iter()
        .position(|event| matches!(event, Ok(ResponseEvent::OutputItemDone(_))))
        .expect("flushed partial Done");
    let error_index = events
        .iter()
        .position(|event| event.is_err())
        .expect("cap error");
    assert!(
        done_index < error_index,
        "the partial must flush before the terminal error: {events:?}"
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
