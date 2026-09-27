use super::error_tests::provider;
use super::support;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
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
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use tokio::time::timeout;

#[derive(Clone, Copy, Debug)]
enum Ending {
    Complete,
    Eof,
    Error,
    Malformed,
}

fn tool_prefix(protocol: RigProtocol) -> String {
    let calls = [
        ("call-function", "lookup", json!({"query":"hello"})),
        ("call-custom", "exec", json!({"input":"echo hello"})),
    ];
    let mut frames = Vec::new();
    match protocol {
        RigProtocol::Chat => {
            for (index, (id, name, arguments)) in calls.into_iter().enumerate() {
                frames.push(json!({"id":"chatcmpl-tools","object":"chat.completion.chunk","created":1,"model":"review-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":null}]}));
            }
            frames.push(json!({"id":"chatcmpl-tools","object":"chat.completion.chunk","created":1,"model":"review-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}));
        }
        RigProtocol::Anthropic => {
            frames.push(json!({"type":"message_start","message":{"id":"msg-tools","type":"message","role":"assistant","content":[],"model":"review-model","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":0}}}));
            for (index, (id, name, arguments)) in calls.into_iter().enumerate() {
                frames.extend([
                    json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
                    json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":arguments.to_string()}}),
                    json!({"type":"content_block_stop","index":index}),
                ]);
            }
            frames.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":2}}));
        }
    }
    frames
        .into_iter()
        .map(|frame| match protocol {
            RigProtocol::Chat => format!("data: {frame}\n\n"),
            RigProtocol::Anthropic => {
                format!(
                    "event: {}\ndata: {frame}\n\n",
                    frame["type"].as_str().unwrap()
                )
            }
        })
        .collect()
}

/// The server writes both complete calls and their finish reason, then waits
/// for the test to explicitly release the provider's final frame or failure.
async fn controlled_stream(
    protocol: RigProtocol,
    ending: Ending,
) -> (
    ResponseStream,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let (prefix_sent, prefix_received) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let server = tokio::spawn(async move {
        timeout(Duration::from_secs(45), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 8192];
                let count = socket.read(&mut bytes).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&bytes[..count]);
                if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..index]).unwrap();
                    let length: usize = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    }).unwrap();
                    if request.len() >= index + 4 + length {
                        break;
                    }
                }
            }
            let prefix = tool_prefix(protocol);
            let tail = match (protocol, ending) {
                (RigProtocol::Chat, Ending::Complete) => "data: [DONE]\n\n",
                (RigProtocol::Anthropic, Ending::Complete) => "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                (_, Ending::Eof) => "",
                (_, Ending::Error) => "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"late failure\"}}\n\n",
                (_, Ending::Malformed) => "data: {broken json\n\n",
            };
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                prefix.len() + tail.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(prefix.as_bytes()).await.unwrap();
            prefix_sent.send(()).unwrap();
            released.await.unwrap();
            socket.write_all(tail.as_bytes()).await.unwrap();
        }).await.expect("controlled HTTP exchange must finish");
    });
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([
            {"type":"function","name":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}}}},
            {"type":"custom","name":"exec"}
        ]),
    );
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    // Client construction can block on platform HTTP/TLS setup; give it a
    // separate deadline from the three-second stream idle bound.
    let stream = timeout(
        Duration::from_secs(30),
        stream_via_rig(
            &request,
            &provider,
            &auth,
            http::HeaderMap::new(),
            protocol,
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("stream start deadline")
    .expect("tool delta starts the stream");
    prefix_received.await.unwrap();
    (stream, release, server)
}

#[tokio::test]
async fn confirmed_function_and_custom_tools_are_not_published_after_terminal_failure() {
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        for ending in [Ending::Eof, Ending::Error, Ending::Malformed] {
            let (mut stream, release, server) = controlled_stream(protocol, ending).await;
            release.send(()).unwrap();
            let mut events = Vec::new();
            let mut errors = 0;
            timeout(Duration::from_secs(3), async {
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(event) => events.push(event),
                        Err(_) => errors += 1,
                    }
                }
            })
            .await
            .expect("terminal failure must close the stream");
            assert_eq!(
                serde_json::to_value(events).unwrap(),
                serde_json::to_value(vec![ResponseEvent::Created { response_id: None }]).unwrap(),
                "no function/custom Added, Delta, Done or Completed for {protocol:?}/{ending:?}"
            );
            assert_eq!(errors, 1, "{protocol:?}/{ending:?}");
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn function_and_custom_lifecycles_wait_for_the_released_terminal_frame() {
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        let (mut stream, release, server) = controlled_stream(protocol, Ending::Complete).await;
        assert!(matches!(
            stream.next().await,
            Some(Ok(ResponseEvent::Created { .. }))
        ));
        // The server has already sent complete tool calls and a finish reason.
        // Keep the final frame gated while polling the actual consumer; a tool
        // published before terminal validation makes this await return early.
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "tool lifecycle escaped before the {protocol:?} terminal was released"
        );
        release.send(()).unwrap();
        let mut events = Vec::new();
        timeout(Duration::from_secs(3), async {
            while let Some(event) = stream.next().await {
                let event = event.expect("successful terminal");
                if !matches!(event, ResponseEvent::ServerModel(_)) {
                    events.push(event);
                }
            }
        })
        .await
        .expect("released terminal must close the stream");

        let ids: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                ResponseEvent::OutputItemAdded(
                    ResponseItem::FunctionCall { id, .. } | ResponseItem::CustomToolCall { id, .. },
                ) => id.as_ref().map(ToString::to_string),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        let mut expected = Vec::new();
        for (id, kind, call_id, name, field, input) in [
            (
                &ids[0],
                "function_call",
                "call-function",
                "lookup",
                "arguments",
                "{\"query\":\"hello\"}",
            ),
            (
                &ids[1],
                "custom_tool_call",
                "call-custom",
                "exec",
                "input",
                "echo hello",
            ),
        ] {
            let mut item = json!({"type":kind,"id":id,"call_id":call_id,"name":name});
            item[field] = Value::String(String::new());
            expected.push(ResponseEvent::OutputItemAdded(
                serde_json::from_value(item.clone()).unwrap(),
            ));
            expected.push(ResponseEvent::ToolCallInputDelta {
                item_id: id.clone(),
                call_id: Some(call_id.into()),
                delta: input.into(),
            });
            item[field] = Value::String(input.into());
            expected.push(ResponseEvent::OutputItemDone(
                serde_json::from_value(item).unwrap(),
            ));
        }
        expected.push(ResponseEvent::Completed {
            response_id: match protocol {
                RigProtocol::Chat => "chatcmpl-tools",
                RigProtocol::Anthropic => "msg-tools",
            }
            .into(),
            token_usage: Some(TokenUsage {
                input_tokens: 4,
                output_tokens: 2,
                total_tokens: 6,
                ..Default::default()
            }),
            usage_metadata: None,
            end_turn: Some(false),
        });
        assert_eq!(
            serde_json::to_value(events).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        server.await.unwrap();
    }
}
