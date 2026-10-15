//! Three-protocol outbound regressions for the final wire budget: a legal
//! inline image must be dispatched (its Base64 payload is transport encoding,
//! not model-visible text), oversized text must still be rejected before any
//! connection, and the bytes that reach the socket must be exactly the
//! serialized request. These tests drive the real transport stack against a
//! local HTTP/1.1 server — no provider, no paid calls.

use crate::RigProtocol;
use crate::wire_budget::WireBudget;
use bytes::Bytes;
use pretty_assertions::assert_eq;
use rig_core::http_client::HttpClientExt;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

type TestError = Box<dyn std::error::Error + Send + Sync>;

/// Local-loopback IO deadline; the negative-control path idles out its full
/// length by design (nothing may ever connect).
const IO_DEADLINE: Duration = Duration::from_secs(5);

/// Headroom for wrapping a task that itself waits out `IO_DEADLINE` (the
/// negative control's no-connection accept window): the wrapper must not
/// race the inner timer to the same instant, or a loaded runtime fails the
/// test before the inner deadline can report its clean `None`. The inner
/// deadline stays the assertion; this only bounds its teardown.
const OUTER_DEADLINE: Duration = Duration::from_secs(10);

const CONTEXT_WINDOW_128K: i64 = 128_000;
const SSE_EMPTY_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 0\r\n\r\n";

fn image_payload(chars: usize) -> String {
    // Valid-alphabet Base64; only the length matters to the budget.
    "QUJD".repeat(chars / 4)
}

/// Reads one full HTTP/1.1 request (headers + Content-Length body) and
/// returns the body bytes.
async fn read_request_body(socket: &mut tokio::net::TcpStream) -> Result<Vec<u8>, TestError> {
    let deadline = IO_DEADLINE;
    let mut received = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let header_end = loop {
        let read = tokio::time::timeout(deadline, socket.read(&mut chunk)).await??;
        if read == 0 {
            return Err("connection closed before request headers completed".into());
        }
        received.extend_from_slice(&chunk[..read]);
        if let Some(position) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&received[..header_end])?.to_ascii_lowercase();
    if headers.contains("expect: 100-continue") {
        socket.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while received.len() < header_end + content_length {
        let read = tokio::time::timeout(deadline, socket.read(&mut chunk)).await??;
        if read == 0 {
            break;
        }
        received.extend_from_slice(&chunk[..read]);
    }
    Ok(received[header_end..].to_vec())
}

/// Accepts one connection, captures its request body, answers with an empty
/// SSE response. Returns `None` when nothing arrived before the deadline.
async fn serve_one_sse(
    listener: TcpListener,
    attempts: Arc<AtomicUsize>,
) -> Result<Option<Vec<u8>>, TestError> {
    let (mut socket, _) = match tokio::time::timeout(IO_DEADLINE, listener.accept()).await {
        Ok(accepted) => accepted?,
        Err(_) => return Ok(None),
    };
    attempts.fetch_add(1, Ordering::SeqCst);
    let body = read_request_body(&mut socket).await?;
    socket.write_all(SSE_EMPTY_RESPONSE).await?;
    Ok(Some(body))
}

/// Accepts connections until `stop` fires; captures each request body in
/// arrival order and answers each with an empty SSE response.
async fn serve_sse_until_stopped(
    listener: TcpListener,
    attempts: Arc<AtomicUsize>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<Vec<Vec<u8>>, TestError> {
    let mut bodies = Vec::new();
    loop {
        let (mut socket, _) = tokio::select! {
            biased;
            _ = stop.changed() => break,
            accepted = tokio::time::timeout(IO_DEADLINE, listener.accept()) => {
                accepted??
            }
        };
        attempts.fetch_add(1, Ordering::SeqCst);
        let body = read_request_body(&mut socket).await?;
        socket.write_all(SSE_EMPTY_RESPONSE).await?;
        bodies.push(body);
    }
    Ok(bodies)
}

struct OutboundOutcome {
    dispatched: Option<Vec<u8>>,
    attempts: usize,
    /// `None` when the transport accepted the request for dispatch.
    api_error: Option<codex_api::ApiError>,
}

async fn dispatch(
    protocol: RigProtocol,
    path: &str,
    body: &Value,
) -> Result<OutboundOutcome, TestError> {
    dispatch_with_window(protocol, path, body, CONTEXT_WINDOW_128K).await
}

async fn dispatch_with_window(
    protocol: RigProtocol,
    path: &str,
    body: &Value,
    context_window_tokens: i64,
) -> Result<OutboundOutcome, TestError> {
    // Native trust-store initialization is synchronous and can be slow on
    // macOS. Arm the server's IO deadline only after client setup finishes.
    let client = crate::transport::RigHttpClient {
        inner: reqwest_rig::Client::builder().no_proxy().build()?,
        protocol,
        wire_budget: WireBudget {
            context_window_tokens: Some(context_window_tokens),
            output_tokens: None,
        },
        ..Default::default()
    };
    let body = Bytes::from(serde_json::to_vec(body)?);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = Arc::clone(&attempts);
    let server = tokio::spawn(async move { serve_one_sse(listener, server_attempts).await });
    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri(format!("http://{address}{path}"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(body)?;
    let outcome = tokio::time::timeout(OUTER_DEADLINE, client.send_streaming(request)).await;
    let server_body = tokio::time::timeout(OUTER_DEADLINE, server).await???;
    Ok(match outcome {
        Ok(Ok(_stream)) => OutboundOutcome {
            dispatched: server_body,
            attempts: attempts.load(Ordering::SeqCst),
            api_error: None,
        },
        Ok(Err(error)) => OutboundOutcome {
            dispatched: server_body,
            attempts: attempts.load(Ordering::SeqCst),
            api_error: crate::wire_budget::api_error(&error),
        },
        Err(_) => return Err("send_streaming timed out".into()),
    })
}

#[tokio::test]
async fn small_original_image_reaches_the_socket_with_a_small_window() -> Result<(), TestError> {
    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    for (protocol, path, body) in [
        (
            RigProtocol::Chat,
            "/v1/chat/completions",
            json!({
                "model":"test-model", "stream":true, "messages":[{"role":"user","content":[
                    {"type":"image_url","image_url":{"url":image_url,"detail":"original"}}
                ]}]
            }),
        ),
        (
            RigProtocol::Responses,
            "/v1/responses",
            json!({
                "model":"test-model", "stream":true, "input":[{"type":"message","role":"user","content":[
                    {"type":"input_image","image_url":image_url,"detail":"original"}
                ]}]
            }),
        ),
    ] {
        let outcome =
            dispatch_with_window(protocol, path, &body, /*context_window_tokens*/ 4_000).await?;
        assert!(
            outcome.api_error.is_none(),
            "small originals keep their true patch cost"
        );
        assert_eq!(outcome.attempts, 1);
        assert_eq!(outcome.dispatched, Some(serde_json::to_vec(&body)?));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_inline_image_is_dispatched_with_unchanged_bytes() -> Result<(), TestError> {
    let payload = image_payload(600_000);
    let body = json!({
        "model": "test-model",
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "describe this screenshot"},
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{payload}"),
                        "detail": "high",
                    },
                },
            ],
        }],
    });
    let outcome = dispatch(crate::RigProtocol::Chat, "/v1/chat/completions", &body).await?;
    assert!(
        outcome.api_error.is_none(),
        "a legal image must not be rejected"
    );
    assert_eq!(outcome.attempts, 1, "exactly one outbound attempt");
    assert_eq!(
        outcome.dispatched.as_deref(),
        Some(serde_json::to_vec(&body)?.as_slice()),
        "the dispatched bytes must be the serialized request, verbatim"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_inline_images_are_dispatched_across_user_and_tool_blocks()
-> Result<(), TestError> {
    let payload = image_payload(300_000);
    let body = json!({
        "model": "test-model",
        "stream": true,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": [
                        {"type": "text", "text": "screenshot attached"},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": payload,
                            },
                        },
                    ],
                },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": payload,
                    },
                },
            ],
        }],
    });
    let outcome = dispatch(crate::RigProtocol::Anthropic, "/v1/messages", &body).await?;
    assert!(
        outcome.api_error.is_none(),
        "legal images must not be rejected"
    );
    assert_eq!(outcome.attempts, 1, "exactly one outbound attempt");
    // The Anthropic path always re-serializes the parsed body (raw-content
    // anchor cleanup); the deterministic round-trip of this input is the
    // expected wire form and still carries both payloads in full.
    let expected = serde_json::to_vec(&serde_json::from_slice::<Value>(&serde_json::to_vec(
        &body,
    )?)?)?;
    assert_eq!(outcome.dispatched, Some(expected));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_user_and_tool_output_images_are_dispatched() -> Result<(), TestError> {
    let payload = image_payload(600_000);
    let body = json!({
        "model": "test-model",
        "stream": true,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "see this"},
                    {
                        "type": "input_image",
                        "image_url": format!("data:image/png;base64,{payload}"),
                        "detail": "high",
                    },
                ],
            },
            {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": [
                    {"type": "input_text", "text": "screenshot attached"},
                    {
                        "type": "input_image",
                        "image_url": format!("data:image/png;base64,{payload}"),
                        "detail": "high",
                    },
                ],
            },
        ],
    });
    let outcome = dispatch(crate::RigProtocol::Responses, "/v1/responses", &body).await?;
    assert!(
        outcome.api_error.is_none(),
        "legal images must not be rejected"
    );
    assert_eq!(outcome.attempts, 1, "exactly one outbound attempt");
    assert_eq!(
        outcome.dispatched.as_deref(),
        Some(serde_json::to_vec(&body)?.as_slice()),
        "the Responses passthrough must send the serialized request verbatim"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_text_is_still_rejected_before_any_connection() -> Result<(), TestError> {
    let text = "x".repeat(600_000);
    let bodies = [
        (
            crate::RigProtocol::Chat,
            json!({
                "model": "test-model",
                "stream": true,
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": text}],
                }],
            }),
        ),
        (
            crate::RigProtocol::Anthropic,
            json!({
                "model": "test-model",
                "stream": true,
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": text}],
                }],
            }),
        ),
        (
            crate::RigProtocol::Responses,
            json!({
                "model": "test-model",
                "stream": true,
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }],
            }),
        ),
    ];
    for (protocol, body) in bodies {
        let outcome = dispatch(protocol, "/v1/test", &body).await?;
        assert!(
            matches!(
                outcome.api_error,
                Some(codex_api::ApiError::ContextWindowExceeded)
            ),
            "{protocol:?}: real text over the window must stay rejected, got {:?}",
            outcome.api_error
        );
        assert_eq!(
            outcome.dispatched, None,
            "{protocol:?}: rejection must happen before any connection"
        );
        assert_eq!(outcome.attempts, 0);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_image_with_larger_encoding_still_fits_the_window() -> Result<(), TestError> {
    // Same single image, two encodings whose Base64 differs 6x in size. The
    // vision cost must not scale with the encoded length, so both requests
    // must dispatch under a window the old whole-body estimate would reject
    // the larger one from.
    let client = crate::transport::RigHttpClient {
        inner: reqwest_rig::Client::builder().no_proxy().build()?,
        protocol: crate::RigProtocol::Chat,
        wire_budget: WireBudget {
            context_window_tokens: Some(30_000),
            output_tokens: None,
        },
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = Arc::clone(&attempts);
    let (stop, stop_receiver) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(async move {
        serve_sse_until_stopped(listener, server_attempts, stop_receiver).await
    });
    let mut expected_bodies = Vec::new();
    for payload in [image_payload(100_000), image_payload(600_000)] {
        let body = json!({
            "model": "test-model",
            "stream": true,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {
                        "type": "image_url",
                        "image_url": {"url": format!("data:image/png;base64,{payload}")},
                    },
                ],
            }],
        });
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri(format!("http://{address}/v1/chat/completions"))
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Bytes::from(serde_json::to_vec(&body)?))?;
        expected_bodies.push(serde_json::to_vec(&body)?);
        let sent = tokio::time::timeout(IO_DEADLINE, client.send_streaming(request))
            .await
            .map_err(|_| "send_streaming timed out")?;
        assert!(
            sent.is_ok(),
            "payload of {} chars must dispatch",
            payload.len()
        );
    }
    stop.send(true)?;
    let bodies = tokio::time::timeout(IO_DEADLINE, server).await???;
    assert_eq!(bodies, expected_bodies, "both payloads reach the socket");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    Ok(())
}
