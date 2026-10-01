use super::support;
use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponseEvent;
use codex_api::ResponseStream;
use codex_api::RetryConfig;
use codex_api::SharedAuthProvider;
use codex_api::TransportError;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use http::StatusCode;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

pub(super) fn provider(address: SocketAddr) -> Provider {
    Provider {
        name: "loopback".into(),
        base_url: format!("http://{address}/v1"),
        query_params: None,
        headers: http::HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(1),
        max_output_tokens: None,
    }
}

enum Reply {
    Status(StatusCode),
    Stall,
    Truncated,
    CompletedThenStall,
    MissingTerminal,
    ErrorAfterFinish,
    Malformed,
    TruncatedStatus,
}

const CHAT_WITH_TRAILING_USAGE: &str = concat!(
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"server-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"完成✅\"},\"finish_reason\":null}]}\r\n\r\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"server-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"server-model\",\r\n",
    "data: \"choices\":[],\"usage\":{\"prompt_tokens\":23,\"completion_tokens\":7,\"total_tokens\":30}}\r\n\r\n",
    "data: [DONE]\r\n\r\n",
);

async fn serve(
    protocol: RigProtocol,
    reply: Reply,
) -> (
    Result<ResponseStream, ApiError>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = provider(listener.local_addr().unwrap());
    provider.query_params = Some(
        [("api_key".into(), "query-secret-not-for-logs".into())]
            .into_iter()
            .collect(),
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buffer = [0; 8192];
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..index]).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= index + 4 + length {
                    break;
                }
            }
        }
        let payload = match protocol {
            RigProtocol::Responses => support::RESPONSES_SSE,
            RigProtocol::Chat => support::CHAT_SSE,
            RigProtocol::Anthropic => support::ANTHROPIC_SSE,
        };
        match reply {
            Reply::TruncatedStatus => {
                socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n{\"error\":").await.unwrap();
            }
            Reply::MissingTerminal | Reply::ErrorAfterFinish | Reply::Malformed => {
                let terminal = match protocol {
                    RigProtocol::Chat => "data: [DONE]\n\n",
                    RigProtocol::Anthropic => {
                        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
                    }
                    RigProtocol::Responses => {
                        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-test\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1,\"total_tokens\":5}}}\n\n"
                    }
                };
                let mut payload = payload.replace(terminal, "");
                if matches!(reply, Reply::ErrorAfterFinish) {
                    payload.push_str("event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"late failure\"}}\n\n");
                }
                if matches!(reply, Reply::Malformed) {
                    payload.push_str("data: {broken json\n\n");
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            Reply::Status(status) => {
                let body =
                    r#"{"error":{"message":"test rejection","type":"invalid_request_error"}}"#;
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nx-request-id: rejected-local\r\nRetry-After: 7\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            Reply::Truncated => {
                let frames = match protocol {
                    RigProtocol::Chat => 1,
                    RigProtocol::Anthropic => 3,
                    RigProtocol::Responses => 2,
                };
                let first = payload
                    .split("\n\n")
                    .take(frames)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{first}\n\n",
                    first.len() + 2
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            Reply::Stall | Reply::CompletedThenStall => {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
                if matches!(reply, Reply::CompletedThenStall) {
                    let payload = match protocol {
                        RigProtocol::Chat => CHAT_WITH_TRAILING_USAGE,
                        RigProtocol::Responses | RigProtocol::Anthropic => payload,
                    };
                    // Split CRLF, JSON, UTF-8 and [DONE] over independent HTTP chunks.
                    for chunk in payload.as_bytes().chunks(7) {
                        socket
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await
                            .unwrap();
                        socket.write_all(chunk).await.unwrap();
                        socket.write_all(b"\r\n").await.unwrap();
                    }
                }
                // Leave HTTP open beyond the bridge idle deadline. A completed
                // model response must close its stream without a late timeout.
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
        }
    });
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let stream = stream_via_rig(
        &support::request(vec![support::user()]),
        &provider,
        &auth,
        http::HeaderMap::new(),
        protocol,
        Duration::from_millis(500),
    )
    .await;
    (stream, server)
}

#[tokio::test]
async fn finish_reason_alone_cannot_hide_truncation_or_late_errors() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        for reply in [
            Reply::MissingTerminal,
            Reply::ErrorAfterFinish,
            Reply::Malformed,
        ] {
            let (result, server) = serve(protocol, reply).await;
            let mut failed = result.is_err();
            if let Ok(mut stream) = result {
                while let Some(event) = stream.next().await {
                    assert!(!matches!(event, Ok(ResponseEvent::Completed { .. })));
                    assert!(!matches!(
                        event,
                        Ok(ResponseEvent::OutputItemDone(
                            codex_protocol::models::ResponseItem::FunctionCall { .. }
                        ))
                    ));
                    failed |= event.is_err();
                }
            }
            assert!(failed, "broken terminal must fail for {protocol:?}");
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn failed_http_error_body_read_preserves_status_without_query_secrets() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let (result, server) = serve(protocol, Reply::TruncatedStatus).await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("expected status failure"),
        };
        let rendered = format!("{error:?} {error}");
        assert!(
            !rendered.contains("query-secret-not-for-logs"),
            "{rendered}"
        );
        assert!(!rendered.contains("api_key="), "{rendered}");
        assert!(matches!(
            error,
            ApiError::Transport(TransportError::Http {
                status: StatusCode::UNAUTHORIZED,
                ..
            })
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn http_status_errors_are_returned_before_a_stream_is_created() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        for expected in [
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let (result, server) = serve(protocol, Reply::Status(expected)).await;
            match result {
                Err(ApiError::Transport(TransportError::Http {
                    status,
                    headers,
                    body,
                    retry_after,
                    ..
                })) => {
                    pretty_assertions::assert_eq!(status, expected);
                    pretty_assertions::assert_eq!(
                        headers.unwrap()["x-request-id"],
                        "rejected-local"
                    );
                    pretty_assertions::assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&body.unwrap()).unwrap(),
                        serde_json::json!({"error":{"message":"test rejection","type":"invalid_request_error"}})
                    );
                    let delay = retry_after.expect("Retry-After retained").remaining_delay();
                    assert!(delay > Duration::from_secs(5) && delay <= Duration::from_secs(7));
                }
                _ => panic!("HTTP status was lost for {protocol:?}: {expected}"),
            }
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn missing_first_event_times_out_during_stream_start() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let (result, server) = serve(protocol, Reply::Stall).await;
        match protocol {
            // The conversion pipeline eagerly resolves the first rig event,
            // so a stalled stream surfaces as a start error.
            RigProtocol::Chat | RigProtocol::Anthropic => {
                assert!(matches!(
                    result,
                    Err(ApiError::Transport(TransportError::Timeout))
                ));
            }
            // The passthrough awaits the HTTP response itself (start errors
            // are HTTP failures) and reports body-level stalls as the first
            // stream event, matching the native Responses transport.
            RigProtocol::Responses => {
                let mut stream = result.expect("headers arrived; stream created");
                let first = stream.next().await;
                assert!(
                    matches!(&first, Some(Err(ApiError::Stream(message))) if message.contains("idle timeout")),
                    "expected an idle-timeout event, got {first:?}"
                );
            }
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn truncated_stream_never_reports_completion() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let (result, server) = serve(protocol, Reply::Truncated).await;
        let mut stream = result.expect("first event");
        let mut failed = false;
        while let Some(event) = stream.next().await {
            assert!(
                !matches!(event, Ok(ResponseEvent::Completed { .. })),
                "truncation was treated as success"
            );
            failed |= event.is_err();
        }
        assert!(failed, "truncation must return an error");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn completion_closes_stream_before_http_idle_timeout() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let (result, server) = serve(protocol, Reply::CompletedThenStall).await;
        let mut stream = result.expect("first event");
        let mut completed = 0;
        let mut text = String::new();
        tokio::time::timeout(Duration::from_millis(1000), async {
            while let Some(event) = stream.next().await {
                match event.expect("no late timeout") {
                    ResponseEvent::OutputTextDelta(delta) => text.push_str(&delta),
                    ResponseEvent::Completed { token_usage, .. } => {
                        completed += 1;
                        // The Anthropic pipeline asserts usage separately
                        // (see the usage suite); here only the streams that
                        // report usage in their terminal frame are checked.
                        if let Some(expected_usage) = match protocol {
                            RigProtocol::Chat => Some((23, 7, 30)),
                            RigProtocol::Responses => Some((4, 1, 5)),
                            RigProtocol::Anthropic => None,
                        } {
                            pretty_assertions::assert_eq!(
                                token_usage,
                                Some(codex_protocol::protocol::TokenUsage {
                                    input_tokens: expected_usage.0,
                                    output_tokens: expected_usage.1,
                                    total_tokens: expected_usage.2,
                                    ..Default::default()
                                })
                            );
                        }
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("model completion must precede HTTP EOF");
        pretty_assertions::assert_eq!(completed, 1);
        pretty_assertions::assert_eq!(
            text,
            match protocol {
                RigProtocol::Chat => "完成✅",
                RigProtocol::Responses | RigProtocol::Anthropic => "ok",
            }
        );
        server.await.unwrap();
    }
}
