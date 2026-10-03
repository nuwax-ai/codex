use super::error_tests::provider;
use super::support;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_api::RetryConfig;
use codex_api::TransportError;
use codex_rust_rig_bridge::FinalRequestCapture;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::RigTurnRecorders;
use codex_rust_rig_bridge::stream_via_rig_with_recorders;
use futures::StreamExt;
use http::StatusCode;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

#[derive(Clone, Copy)]
enum Reply {
    Status(StatusCode),
    DroppedConnection,
    Complete,
    Truncated,
}

fn policy(retries: u64) -> RetryConfig {
    RetryConfig {
        max_attempts: retries,
        base_delay: Duration::ZERO,
        retry_429: false,
        retry_5xx: true,
        retry_transport: true,
    }
}

fn payload(protocol: RigProtocol) -> &'static str {
    match protocol {
        RigProtocol::Responses => support::RESPONSES_SSE,
        RigProtocol::Chat => support::CHAT_SSE,
        RigProtocol::Anthropic => support::ANTHROPIC_SSE,
    }
}

struct Outcome {
    wires: Vec<Value>,
    times: Vec<Instant>,
    errors: Vec<ApiError>,
    completed: usize,
    capture: FinalRequestCapture,
}

async fn run(protocol: RigProtocol, replies: Vec<Reply>, retry: RetryConfig) -> Outcome {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let mut provider = provider(listener.local_addr().expect("address"));
    provider.retry = retry;
    let server = tokio::spawn(async move {
        let mut wires = Vec::new();
        let mut times = Vec::new();
        for reply in replies {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(15), listener.accept())
                .await
                .expect("attempt deadline")
                .expect("accept");
            let request = support::read_request(&mut socket).await;
            wires.push(request);
            times.push(Instant::now());
            if matches!(reply, Reply::DroppedConnection) {
                continue;
            }
            let (status, content_type, body) = match reply {
                Reply::Status(status) => (
                    status,
                    "application/json",
                    "{\"error\":{\"message\":\"retry me\"}}",
                ),
                Reply::Complete => (StatusCode::OK, "text/event-stream", payload(protocol)),
                Reply::Truncated => (
                    StatusCode::OK,
                    "text/event-stream",
                    payload(protocol).split_once("\n\n").expect("first frame").0,
                ),
                Reply::DroppedConnection => unreachable!("handled above"),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nx-request-id: retry-test\r\nRetry-After: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.expect("reply");
        }
        (wires, times)
    });
    let capture = Arc::new(Mutex::new(FinalRequestCapture::default()));
    let request = support::request(vec![support::user()]);
    let auth: codex_api::SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut errors = Vec::new();
    let mut completed = 0;
    match stream_via_rig_with_recorders(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        protocol,
        Duration::from_secs(5),
        RigTurnRecorders {
            final_request: Some(capture.clone()),
            ..Default::default()
        },
    )
    .await
    {
        Ok((mut stream, _)) => {
            while let Some(event) = stream.next().await {
                match event {
                    Ok(ResponseEvent::Completed { .. }) => completed += 1,
                    Err(error) => errors.push(error),
                    Ok(_) => {}
                }
            }
        }
        Err(error) => errors.push(error),
    }
    let (wires, times) = server.await.expect("server");
    let capture = capture.lock().expect("capture").clone();
    Outcome {
        wires,
        times,
        errors,
        completed,
        capture,
    }
}

#[tokio::test]
async fn http_retry_reuses_final_request_bytes_and_captures_every_attempt_on_all_wires() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let result = run(
            protocol,
            vec![
                Reply::Status(StatusCode::SERVICE_UNAVAILABLE),
                Reply::Complete,
            ],
            policy(/*retries*/ 1),
        )
        .await;
        assert_eq!(
            (
                result.wires.len(),
                result.capture.requests.len(),
                result.completed,
                result.errors.len()
            ),
            (2, 2, 1, 0)
        );
        assert_eq!(result.wires[0], result.wires[1]);
        assert_eq!(
            result.capture.requests[0].body_raw,
            result.wires[0]["body_raw"].as_str().expect("body")
        );
        assert_eq!(
            result.capture.requests[0].body_raw,
            result.capture.requests[1].body_raw
        );
        assert!(
            result.times[1].duration_since(result.times[0]) >= Duration::from_secs(1),
            "Retry-After must govern actual HTTP attempts"
        );
    }
}

#[tokio::test]
async fn http_retry_zero_disabled_and_exhaustion_keep_the_last_http_error() {
    for (retry, replies, count) in [
        (
            policy(/*retries*/ 0),
            vec![Reply::Status(StatusCode::SERVICE_UNAVAILABLE)],
            1,
        ),
        (
            RetryConfig {
                retry_5xx: false,
                ..policy(/*retries*/ 2)
            },
            vec![Reply::Status(StatusCode::SERVICE_UNAVAILABLE)],
            1,
        ),
        (
            policy(/*retries*/ 1),
            vec![Reply::Status(StatusCode::SERVICE_UNAVAILABLE); 2],
            2,
        ),
    ] {
        let result = run(RigProtocol::Responses, replies, retry).await;
        assert_eq!(
            (
                result.wires.len(),
                result.capture.requests.len(),
                result.completed
            ),
            (count, count, 0)
        );
        assert!(matches!(
            result.errors.as_slice(),
            [ApiError::Transport(TransportError::Http {
                status: StatusCode::SERVICE_UNAVAILABLE,
                ..
            })]
        ));
    }
}

#[tokio::test]
async fn http_retry_429_obeys_the_provider_flag() {
    for retry_429 in [false, true] {
        let mut replies = vec![Reply::Status(StatusCode::TOO_MANY_REQUESTS)];
        if retry_429 {
            replies.push(Reply::Complete);
        }
        let result = run(
            RigProtocol::Chat,
            replies,
            RetryConfig {
                retry_429,
                ..policy(/*retries*/ 1)
            },
        )
        .await;
        assert_eq!(
            (result.wires.len(), result.completed, result.errors.len()),
            if retry_429 { (2, 1, 0) } else { (1, 0, 1) }
        );
    }
}

#[tokio::test]
async fn http_retry_transport_obeys_the_provider_flag() {
    for retry_transport in [false, true] {
        let mut replies = vec![Reply::DroppedConnection];
        if retry_transport {
            replies.push(Reply::Complete);
        }
        let result = run(
            RigProtocol::Anthropic,
            replies,
            RetryConfig {
                retry_transport,
                ..policy(/*retries*/ 1)
            },
        )
        .await;
        assert_eq!(
            (result.wires.len(), result.completed, result.errors.len()),
            if retry_transport {
                (2, 1, 0)
            } else {
                (1, 0, 1)
            }
        );
    }
}

#[tokio::test]
async fn http_retry_never_restarts_a_successful_handshake_with_a_broken_stream() {
    for protocol in [
        RigProtocol::Responses,
        RigProtocol::Chat,
        RigProtocol::Anthropic,
    ] {
        let result = run(protocol, vec![Reply::Truncated], policy(/*retries*/ 3)).await;
        assert_eq!(
            (
                result.wires.len(),
                result.capture.requests.len(),
                result.completed,
                result.errors.len()
            ),
            (1, 1, 0, 1)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_retry_cancellation_during_server_advice_leaves_no_background_retry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let mut provider = provider(listener.local_addr().expect("address"));
    provider.retry = policy(/*retries*/ 3);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        support::read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.expect("reply");
        ready_tx.send(()).expect("notify");
        assert!(
            tokio::time::timeout(Duration::from_millis(1200), listener.accept())
                .await
                .is_err(),
            "cancelled request must not retry after the advice expires"
        );
    });
    let query = tokio::spawn(async move {
        let auth: codex_api::SharedAuthProvider = Arc::new(support::DummyAuth);
        stream_via_rig_with_recorders(
            &support::request(vec![support::user()]),
            &provider,
            &auth,
            http::HeaderMap::new(),
            RigProtocol::Responses,
            Duration::from_secs(5),
            RigTurnRecorders::default(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(15), ready_rx)
        .await
        .expect("first request")
        .expect("ready");
    query.abort();
    assert!(matches!(query.await, Err(error) if error.is_cancelled()));
    server.await.expect("server");
}
