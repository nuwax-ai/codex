use super::error_tests::provider;
use super::support;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_protocol::protocol::TokenUsage;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::replay_rig_events;
use codex_rust_rig_bridge::stream_via_rig_with_recording;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

async fn anthropic_usage(deltas: Vec<Value>) -> TokenUsage {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut frames = vec![json!({"type":"message_start","message":{
            "id":"msg-usage","type":"message","role":"assistant","content":[],
            "model":"review-model","stop_reason":null,"stop_sequence":null,
            "usage":{"input_tokens":4,"output_tokens":0,"cache_read_input_tokens":70,"cache_creation_input_tokens":20}
        }})];
        let count = deltas.len();
        for (index, usage) in deltas.into_iter().enumerate() {
            let stop_reason = (index + 1 == count).then_some("end_turn");
            frames.push(json!({"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},"usage":usage}));
        }
        frames.push(json!({"type":"message_stop"}));
        let payload: String = frames
            .into_iter()
            .map(|frame| {
                format!(
                    "event: {}\ndata: {frame}\n\n",
                    frame["type"].as_str().unwrap()
                )
            })
            .collect();
        support::serve_payload(&listener, &payload).await
    });
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let recorder = Arc::new(std::sync::Mutex::new(Default::default()));
    let (mut stream, _) = stream_via_rig_with_recording(
        &support::request(vec![support::user()]),
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(3),
        Some(recorder.clone()),
    )
    .await
    .unwrap();
    let mut usage = None;
    while let Some(event) = stream.next().await {
        if let ResponseEvent::Completed { token_usage, .. } = event.unwrap() {
            usage = token_usage;
        }
    }
    server.await.unwrap();
    let replay = replay_rig_events(&recorder.lock().unwrap().clone(), Default::default())
        .into_result()
        .unwrap();
    let replay_usage = replay.into_iter().find_map(|event| match event {
        ResponseEvent::Completed { token_usage, .. } => token_usage,
        _ => None,
    });
    assert_eq!(usage, replay_usage, "recording must retain corrected usage");
    usage.expect("completed token usage")
}

#[tokio::test]
async fn anthropic_usage_keeps_cache_counts_only_reported_at_start() {
    assert_eq!(
        anthropic_usage(vec![json!({"output_tokens":3})]).await,
        TokenUsage {
            input_tokens: 94,
            cached_input_tokens: 70,
            cache_write_input_tokens: 20,
            output_tokens: 3,
            total_tokens: 97,
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn anthropic_usage_explicit_zero_overwrites_prior_counters() {
    assert_eq!(
        anthropic_usage(vec![json!({"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":3})]).await,
        TokenUsage {
            output_tokens: 3,
            total_tokens: 3,
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn anthropic_usage_keeps_intermediate_cumulative_updates() {
    assert_eq!(
        anthropic_usage(vec![
            json!({"input_tokens":8,"cache_read_input_tokens":90,"cache_creation_input_tokens":30,"output_tokens":2,"output_tokens_details":{"thinking_tokens":1}}),
            json!({"output_tokens":3,"cache_read_input_tokens":null}),
        ]).await,
        TokenUsage {
            input_tokens: 128,
            cached_input_tokens: 90,
            cache_write_input_tokens: 30,
            output_tokens: 3,
            reasoning_output_tokens: 1,
            total_tokens: 131,
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn anthropic_usage_terminal_counters_are_authoritative() {
    assert_eq!(
        anthropic_usage(vec![json!({"input_tokens":8,"cache_read_input_tokens":90,"cache_creation_input_tokens":30,"output_tokens":7,"output_tokens_details":{"thinking_tokens":2}})]).await,
        TokenUsage {
            input_tokens: 128,
            cached_input_tokens: 90,
            cache_write_input_tokens: 30,
            output_tokens: 7,
            reasoning_output_tokens: 2,
            total_tokens: 135,
            ..Default::default()
        }
    );
}

/// Anthropic requires the usage object on message_start and message_delta.
/// A frame without it is a truncated/malformed reply the bridge must fail
/// (retryable for core) — rig's lenient reader would fold a default terminal.
/// The message_start failure surfaces as a stream START error (the bridge
/// eagerly resolves the first frame); the message_delta failure arrives
/// mid-stream after partial text.
#[tokio::test]
async fn anthropic_missing_usage_fails_the_stream_before_a_terminal() {
    async fn run(
        start_usage: Option<Value>,
        delta_usage: Option<Value>,
    ) -> Vec<Result<ResponseEvent, codex_api::ApiError>> {
        let mut message = json!({
            "id":"msg-missing","type":"message","role":"assistant","content":[],
            "model":"review-model","stop_reason":null,"stop_sequence":null
        });
        if let Some(usage) = start_usage {
            message["usage"] = usage;
        }
        let mut delta = json!({"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null}});
        if let Some(usage) = delta_usage {
            delta["usage"] = usage;
        } else {
            delta.as_object_mut().unwrap().remove("usage");
        }
        let frames = vec![
            json!({"type":"message_start","message":message}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}),
            delta,
            json!({"type":"message_stop"}),
        ];
        let payload: String = frames
            .into_iter()
            .map(|frame| {
                format!(
                    "event: {}\ndata: {frame}\n\n",
                    frame["type"].as_str().unwrap()
                )
            })
            .collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider = provider(listener.local_addr().unwrap());
        let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
        let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
        // A malformed FIRST frame fails the eager start; a malformed later
        // frame fails inside the returned stream. Both must carry the wire
        // requirement through the provider error mapping.
        match stream_via_rig_with_recording(
            &support::request(vec![support::user()]),
            &provider,
            &auth,
            http::HeaderMap::new(),
            RigProtocol::Anthropic,
            Duration::from_secs(3),
            None,
        )
        .await
        {
            Ok((mut stream, _)) => {
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    events.push(event);
                }
                server.await.unwrap();
                events
            }
            Err(error) => vec![Err(error)],
        }
    }

    for (start, delta, label) in [
        (None, Some(json!({"output_tokens":64})), "message_start"),
        (
            Some(json!({"input_tokens":4,"output_tokens":0})),
            None,
            "message_delta",
        ),
    ] {
        let events = run(start, delta).await;
        let errors: Vec<_> = events
            .iter()
            .filter_map(|event| event.as_ref().err())
            .collect();
        assert_eq!(errors.len(), 1, "{label}: {events:?}");
        let message = match &errors[0] {
            codex_api::ApiError::Transport(codex_api::TransportError::Network(message)) => {
                message.clone()
            }
            other => panic!("{label}: expected a retryable transport error, got {other:?}"),
        };
        assert!(
            message.contains("missing field `usage`"),
            "{label}: the wire requirement survives the provider error mapping: {message}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. }))),
            "{label}: no terminal may be folded from a malformed stream"
        );
    }
}
