//! Three-protocol outbound regressions for the final wire budget: a legal
//! inline image must be dispatched (its Base64 payload is transport encoding,
//! not model-visible text), oversized text must still be rejected before any
//! connection, and the bytes that reach the socket must be exactly the
//! serialized request. These tests drive the real transport stack against a
//! local HTTP/1.1 server — no provider, no paid calls.

use crate::RigProtocol;
use crate::wire_budget::WireBudget;
use bytes::Bytes;
use futures::StreamExt;
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
            vision_floor_tokens: 0,
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
            vision_floor_tokens: 0,
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

// ---------------------------------------------------------------------------
// Pre-conversion vision floor through the real product path: a Codex
// request carrying an original-detail image is converted (Chat collapses
// `original` to `high`, Anthropic drops the flag), serialized by the real
// rig SDK, and measured by the transport — the canonical patch estimate
// must survive as the outbound floor. These drive
// `stream_via_rig_with_context` against a local server: no provider, no
// paid calls.
// ---------------------------------------------------------------------------

/// A 45-byte PNG whose IHDR declares 2560×2048: 80×64 = 5120 patch tokens
/// pre-conversion versus the 1844 prepared-image estimate the converted
/// wire block alone would suggest.
const ORIGINAL_2560X2048: &str =
    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAACgAAAAgACAYAAADqy2rxAAAAAElFTkSuQmCC";

const FLOOR_CHAT_SSE: &str = "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1,\"total_tokens\":5}}\n\ndata: [DONE]\n\n";
const FLOOR_ANTHROPIC_SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

struct FloorAuth;
impl codex_api::AuthProvider for FloorAuth {
    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer floor-test-dummy"),
        );
    }
}

fn floor_request(input: Vec<serde_json::Value>, with_tool: bool) -> codex_api::ResponsesApiRequest {
    let mut request = codex_api::ResponsesApiRequest {
        model: "floor-model".into(),
        stream: true,
        service_tier: None,
        input: input
            .into_iter()
            .map(|item| serde_json::from_value(item).expect("fixture item"))
            .collect(),
        tools: None,
        tool_choice: "auto".into(),
        parallel_tool_calls: false,
        max_output_tokens: Some(64),
        reasoning: None,
        store: false,
        stream_options: None,
        include: Vec::new(),
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    };
    if with_tool {
        let raw: Arc<serde_json::value::RawValue> = Arc::from(
            serde_json::value::to_raw_value(&json!([{
                "type":"function","name":"view_image","description":"view",
                "parameters":{"type":"object","properties":{}}
            }]))
            .expect("tools"),
        );
        request.tools = Some(raw.into());
    }
    request
}

fn floor_user_image_items() -> Vec<serde_json::Value> {
    vec![json!({
        "type":"message","role":"user","content":[
            {"type":"input_text","text":"describe the screenshot"},
            {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"}
        ]
    })]
}

fn floor_tool_image_items() -> Vec<serde_json::Value> {
    vec![
        json!({"type":"message","role":"user","content":[
            {"type":"input_text","text":"shoot the page"}
        ]}),
        json!({"type":"function_call","name":"view_image","call_id":"c1","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"c1","output":[
            {"type":"input_text","text":"screenshot attached"},
            {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"}
        ]}),
    ]
}

fn floor_provider(address: std::net::SocketAddr) -> codex_api::Provider {
    codex_api::Provider {
        name: "floor test".into(),
        base_url: format!("http://{address}/v1"),
        query_params: None,
        headers: http::HeaderMap::new(),
        retry: codex_api::RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(3),
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

/// Drives the full conversion + SDK + transport path under a context window
/// and reports exactly what reached the local socket. `accept_window` bounds
/// how long the server listens: rejections return before any network IO, so
/// a short window proves zero attempts; dispatched requests need room for
/// the client's synchronous setup (native trust-store init can be slow on
/// macOS — see `dispatch_with_window`), so positives pass `IO_DEADLINE`.
async fn stream_under_window(
    protocol: RigProtocol,
    request: &codex_api::ResponsesApiRequest,
    context_window_tokens: i64,
    accept_window: Duration,
) -> Result<OutboundOutcome, TestError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let payload = match protocol {
        RigProtocol::Chat => FLOOR_CHAT_SSE,
        RigProtocol::Anthropic => FLOOR_ANTHROPIC_SSE,
        RigProtocol::Responses => unreachable!("floor cases cover the converting wires"),
    }
    .to_string();
    let server = tokio::spawn(async move {
        let mut served = Vec::new();
        let accepted = match tokio::time::timeout(accept_window, listener.accept()).await {
            Ok(Ok((socket, _))) => socket,
            _ => return served,
        };
        let mut socket = accepted;
        if let Ok(body) = read_request_body(&mut socket).await {
            use tokio::io::AsyncWriteExt;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{payload}",
                payload.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            served.push(body);
        }
        served
    });
    let provider = floor_provider(address);
    let auth: codex_api::SharedAuthProvider = Arc::new(FloorAuth);
    let result = crate::stream::stream_via_rig_with_context(
        request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        protocol,
        Duration::from_secs(5),
        crate::RigTurnRecorders::default(),
        crate::wire_budget::RigCallContext {
            context_window_tokens: Some(context_window_tokens),
            ..Default::default()
        },
    )
    .await;
    let (api_error, dispatched) = match result {
        Ok((mut stream, _)) => {
            // Drain the scripted SSE to completion; event payloads are not
            // under test here — the dispatched bytes are.
            while let Some(event) = stream.next().await {
                let _ = event;
            }
            (None, server.await?)
        }
        Err(error) => (Some(error), server.await?),
    };
    let attempts = if dispatched.is_empty() { 0 } else { 1 };
    Ok(OutboundOutcome {
        dispatched: dispatched.into_iter().next(),
        attempts,
        api_error,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn converted_originals_are_rejected_before_http_on_small_windows() -> Result<(), TestError> {
    // Window 3000: the converted wire block alone suggests 1844 + overhead,
    // which would fit; the pre-conversion 5120 must reject instead. User and
    // tool images, Chat and Anthropic.
    for (protocol, with_tool) in [
        (RigProtocol::Chat, false),
        (RigProtocol::Chat, true),
        (RigProtocol::Anthropic, false),
        (RigProtocol::Anthropic, true),
    ] {
        let request = floor_request(
            if with_tool {
                floor_tool_image_items()
            } else {
                floor_user_image_items()
            },
            with_tool,
        );
        let outcome =
            stream_under_window(protocol, &request, 3_000, Duration::from_millis(750)).await?;
        assert!(
            matches!(
                outcome.api_error,
                Some(codex_api::ApiError::ContextWindowExceeded)
            ),
            "{protocol:?} tool={with_tool}: expected a pre-network context rejection, got {:?}",
            outcome.api_error
        );
        assert_eq!(
            outcome.dispatched, None,
            "{protocol:?} tool={with_tool}: rejection must happen before any connection"
        );
        assert_eq!(outcome.attempts, 0);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn converted_originals_reach_the_socket_with_window_room() -> Result<(), TestError> {
    // Window 8000: the floored estimate (5120 + text + output reserve) fits
    // and the image itself must arrive on the wire, in full, as the block
    // its protocol actually uses.
    for (protocol, with_tool) in [
        (RigProtocol::Chat, false),
        (RigProtocol::Chat, true),
        (RigProtocol::Anthropic, false),
        (RigProtocol::Anthropic, true),
    ] {
        let request = floor_request(
            if with_tool {
                floor_tool_image_items()
            } else {
                floor_user_image_items()
            },
            with_tool,
        );
        let outcome = stream_under_window(protocol, &request, 8_000, IO_DEADLINE).await?;
        assert!(
            outcome.api_error.is_none(),
            "{protocol:?} tool={with_tool}: a floored-but-legal request must dispatch, got {:?}",
            outcome.api_error
        );
        let body = outcome
            .dispatched
            .map(|bytes| serde_json::from_slice::<Value>(&bytes))
            .transpose()?
            .expect("dispatched body");
        // Chat keeps the full data URL; Anthropic splits it into a base64
        // source, so assert on the payload half both wires carry in full.
        let image_payload = ORIGINAL_2560X2048
            .rsplit_once(',')
            .expect("data URL payload")
            .1;
        let encoded = body.to_string();
        assert!(
            encoded.contains(image_payload),
            "{protocol:?} tool={with_tool}: the original image payload must reach the socket in full"
        );
        let plain = encoded.replace('\\', "");
        match protocol {
            RigProtocol::Chat => {
                assert!(
                    plain.contains(r#""detail":"high""#),
                    "{protocol:?} tool={with_tool}: Chat renders the converted original as high"
                );
                assert!(
                    !plain.contains(r#""detail":"original""#),
                    "{protocol:?} tool={with_tool}: original detail must not leak to Chat"
                );
            }
            RigProtocol::Anthropic => {
                assert!(
                    plain.contains(r#""source":{"type":"base64""#),
                    "{protocol:?} tool={with_tool}: Anthropic carries the payload as a base64 source"
                );
            }
            RigProtocol::Responses => unreachable!(),
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_originals_sum_the_floor_and_mixed_text_still_counts() -> Result<(), TestError> {
    // Two originals plus real text: floor = 2×5120. Window 6000 fits the
    // naive converted estimate (2×1844 + text) but must reject on the sum;
    // window 12000 dispatches with both payloads.
    let items = || {
        vec![json!({
            "type":"message","role":"user","content":[
                {"type":"input_text","text":"compare these two screenshots carefully"},
                {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"},
                {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"}
            ]
        })]
    };
    let rejected = stream_under_window(
        RigProtocol::Chat,
        &floor_request(items(), false),
        6_000,
        Duration::from_millis(750),
    )
    .await?;
    assert!(
        matches!(
            rejected.api_error,
            Some(codex_api::ApiError::ContextWindowExceeded)
        ),
        "two originals must sum their canonical estimates, got {:?}",
        rejected.api_error
    );
    assert_eq!(rejected.attempts, 0);
    let dispatched = stream_under_window(
        RigProtocol::Chat,
        &floor_request(items(), false),
        12_000,
        IO_DEADLINE,
    )
    .await?;
    assert!(dispatched.api_error.is_none());
    let body = dispatched.dispatched.expect("dispatched body");
    let occurrences = String::from_utf8_lossy(&body)
        .matches(ORIGINAL_2560X2048)
        .count();
    assert_eq!(occurrences, 2, "both images reach the socket");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compacted_text_only_history_carries_no_vision_floor() -> Result<(), TestError> {
    // A post-compaction rebuild drops its images: the summary must dispatch
    // under a window far smaller than any original's floor.
    let items = vec![json!({
        "type":"message","role":"user","content":[
            {"type":"input_text","text":"Compaction summary: the screenshot exchange completed. Reply with exactly: ok"}
        ]
    })];
    let request = floor_request(items, false);
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        let outcome = stream_under_window(protocol, &request, 1_200, IO_DEADLINE).await?;
        assert!(
            outcome.api_error.is_none(),
            "{protocol:?}: the compacted turn must dispatch as plain text, got {:?}",
            outcome.api_error
        );
        let body = outcome.dispatched.expect("dispatched body");
        assert!(
            String::from_utf8_lossy(&body).contains("Compaction summary"),
            "{protocol:?}: the compacted text reaches the socket"
        );
    }
    Ok(())
}

/// One retained original (5120 by patch count) plus one ordinary `high`
/// image (1844). Chat collapses the original to `high` and Anthropic carries
/// no detail flag, so the serialized body prices both at the prepared
/// estimate: walk = 3688. The complete pre-conversion total is 6964, but a
/// floor that accumulates originals alone yields `max(3688, 5120)` = 5120 and
/// deletes the ordinary image's contribution from the budget entirely.
fn floor_mixed_user_items() -> Vec<serde_json::Value> {
    vec![json!({
        "type":"message","role":"user","content":[
            {"type":"input_text","text":"compare the two screenshots"},
            {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"},
            {"type":"input_image","image_url":mixed_high_image(),"detail":"high"}
        ]
    })]
}

/// The same mix reached through the other source the bridge sends images
/// from: an ordinary user image plus an original inside a tool result.
fn floor_mixed_tool_items() -> Vec<serde_json::Value> {
    vec![
        json!({"type":"message","role":"user","content":[
            {"type":"input_text","text":"shoot the page"},
            {"type":"input_image","image_url":mixed_high_image(),"detail":"high"}
        ]}),
        json!({"type":"function_call","name":"view_image","call_id":"c1","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"c1","output":[
            {"type":"input_text","text":"screenshot attached"},
            {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"}
        ]}),
    ]
}

fn mixed_high_image() -> String {
    format!("data:image/png;base64,{}", image_payload(20_000))
}

/// 5120 (original) + 1844 (ordinary high) = 6964 canonical vision tokens.
/// A window of 6000 sits above the buggy `max(3688, 5120)` charge — which
/// also carries the ~100 text tokens and the 64-token output reserve — and
/// below the correct charge, so the request must be refused pre-network.
const MIXED_REJECT_WINDOW: i64 = 6_000;
/// Comfortably above 6964 + text + output reserve.
const MIXED_DISPATCH_WINDOW: i64 = 12_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_original_and_ordinary_images_reject_before_http() -> Result<(), TestError> {
    for (protocol, items, source) in [
        (RigProtocol::Chat, floor_mixed_user_items(), "user"),
        (RigProtocol::Anthropic, floor_mixed_user_items(), "user"),
        (RigProtocol::Chat, floor_mixed_tool_items(), "tool"),
        (RigProtocol::Anthropic, floor_mixed_tool_items(), "tool"),
    ] {
        let with_tool = source == "tool";
        let outcome = stream_under_window(
            protocol,
            &floor_request(items, with_tool),
            MIXED_REJECT_WINDOW,
            Duration::from_millis(750),
        )
        .await?;
        assert!(
            matches!(
                outcome.api_error,
                Some(codex_api::ApiError::ContextWindowExceeded)
            ),
            "{protocol:?} {source}: an original beside an ordinary image must \
             sum both costs and reject, got {:?}",
            outcome.api_error
        );
        assert_eq!(
            outcome.dispatched, None,
            "{protocol:?} {source}: rejection must happen before any connection"
        );
        assert_eq!(outcome.attempts, 0);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_original_and_ordinary_images_reach_the_socket_together() -> Result<(), TestError> {
    // The fix must not drop, downgrade, or re-encode anything: both images
    // arrive complete on a window the correct charge fits.
    let high_payload = image_payload(20_000);
    let original_payload = ORIGINAL_2560X2048
        .rsplit_once(',')
        .expect("data URL payload")
        .1;
    for (protocol, items, source) in [
        (RigProtocol::Chat, floor_mixed_user_items(), "user"),
        (RigProtocol::Anthropic, floor_mixed_user_items(), "user"),
        (RigProtocol::Chat, floor_mixed_tool_items(), "tool"),
        (RigProtocol::Anthropic, floor_mixed_tool_items(), "tool"),
    ] {
        let with_tool = source == "tool";
        let outcome = stream_under_window(
            protocol,
            &floor_request(items, with_tool),
            MIXED_DISPATCH_WINDOW,
            IO_DEADLINE,
        )
        .await?;
        assert!(
            outcome.api_error.is_none(),
            "{protocol:?} {source}: a correctly-priced mix must still dispatch, got {:?}",
            outcome.api_error
        );
        let body = String::from_utf8(outcome.dispatched.expect("dispatched body"))?;
        assert_eq!(
            body.matches(&high_payload).count(),
            1,
            "{protocol:?} {source}: the ordinary image must reach the socket in full"
        );
        assert_eq!(
            body.matches(original_payload).count(),
            1,
            "{protocol:?} {source}: the original image must reach the socket in full"
        );
    }
    Ok(())
}
