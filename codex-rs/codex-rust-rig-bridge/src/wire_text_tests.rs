//! Final HTTP-body contracts, including Rig's consecutive-user adaptation.

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
use tokio::net::TcpStream;
use tokio::sync::oneshot;

// Local loopback only; no provider model or external network is involved.
const DEADLINE: Duration = Duration::from_secs(10);
const EMPTY_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
const CAPTURE_RESPONSE: &[u8] =
    b"HTTP/1.1 418 Capture Complete\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
type TestError = Box<dyn std::error::Error + Send + Sync>;

async fn read_body(socket: &mut TcpStream) -> Result<Vec<u8>, TestError> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err("request ended before its headers".into());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let length: usize = std::str::from_utf8(&bytes[..header_end])?
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim())
        })
        .ok_or("missing content length")?
        .parse()?;
    while bytes.len() < header_end + length {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err("request body ended before its declared content length".into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body = bytes[header_end..].to_vec();
    assert_eq!(
        body.len(),
        length,
        "Content-Length must describe the rewritten body"
    );
    Ok(body)
}

async fn capture_one(
    listener: TcpListener,
    mut stop: oneshot::Receiver<()>,
    attempts: Arc<AtomicUsize>,
    response: &'static [u8],
) -> Result<Option<Vec<u8>>, TestError> {
    // A pending connection wins over stop, so rejection cannot hide an
    // already-established HTTP attempt.
    let (mut socket, _) = tokio::select! {
        biased;
        accepted = listener.accept() => accepted?,
        _ = &mut stop => return Ok(None),
    };
    attempts.fetch_add(1, Ordering::SeqCst);
    let body = read_body(&mut socket).await?;
    socket.write_all(response).await?;
    Ok(Some(body))
}

struct Outcome {
    body: Option<Vec<u8>>,
    attempts: usize,
    error: Option<String>,
    budget_error: Option<codex_api::ApiError>,
}

async fn dispatch(
    protocol: RigProtocol,
    body: &Value,
    budget: WireBudget,
) -> Result<Outcome, TestError> {
    let client = super::RigHttpClient {
        inner: reqwest_rig::Client::builder().no_proxy().build()?,
        protocol,
        wire_budget: budget,
        ..Default::default()
    };
    let raw = serde_json::to_vec_pretty(body)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(capture_one(
        listener,
        stopped,
        Arc::clone(&attempts),
        EMPTY_RESPONSE,
    ));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://{address}/test"))
        .header(http::header::CONTENT_TYPE, "application/json")
        // Deliberately supply the original length: a body rewrite must remove
        // it and let the HTTP client compute the new, shorter length.
        .header(http::header::CONTENT_LENGTH, raw.len().to_string())
        .body(Bytes::from(raw))?;
    let result = tokio::time::timeout(DEADLINE, client.send_streaming(request)).await?;
    let error = result.err();
    let _ = stop.send(());
    let body = tokio::time::timeout(DEADLINE, server).await???;
    Ok(Outcome {
        body,
        attempts: attempts.load(Ordering::SeqCst),
        budget_error: error
            .as_ref()
            .and_then(|error| crate::wire_budget::api_error(error)),
        error: error.map(|error| error.to_string()),
    })
}

#[tokio::test]
async fn chat_plain_text_arrays_reach_http_in_order_without_changing_other_fields()
-> Result<(), TestError> {
    let body = json!({"model":"m","stream":true,"messages":[
        {"role":"system","content":[{"type":"text","text":"rules"}]},
        {"role":"user","content":[{"type":"text","text":"old"},{"type":"text","text":"最新指令"}]},
        {"role":"assistant","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],
         "reasoning_content":"keep reasoning", "tool_calls":[{"id":"call","type":"function","function":{"name":"shell","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call","content":[{"type":"text","text":""},{"type":"text","text":"tool result"}]}
    ]});
    let mut expected = body.clone();
    for (index, text) in ["rules", "old\n\n最新指令", "a\n\nb", "\n\ntool result"]
        .into_iter()
        .enumerate()
    {
        expected["messages"][index]["content"] = text.into();
    }
    let result = dispatch(RigProtocol::Chat, &body, WireBudget::default()).await?;
    assert_eq!(result.error, None);
    assert_eq!(result.attempts, 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&result.body.ok_or("no HTTP body")?)?,
        expected
    );
    Ok(())
}

#[tokio::test]
async fn chat_rich_extended_unknown_and_empty_content_reaches_http_unchanged()
-> Result<(), TestError> {
    let body = json!({"messages":[
        {"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"https://example.invalid/image.png","detail":"high"}}]},
        {"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b","cache_control":{"type":"ephemeral"}}]},
        {"role":"user","content":[{"type":"future","value":"a"}]},
        {"role":"user","content":[]},
        {"role":"user","content":[{"type":"text","text":7}]},
        {"role":"user","content":[{"type":"text","text":"a"},"unknown"]},
        {"role":"user","content":[{"type":"text","text":"a","extension":null}]},
        {"role":"user","content":"already plain"},
        {"role":"user","content":null}
    ]});
    let result = dispatch(RigProtocol::Chat, &body, WireBudget::default()).await?;
    assert_eq!(result.error, None);
    assert_eq!(result.body, Some(serde_json::to_vec_pretty(&body)?));
    Ok(())
}

#[tokio::test]
async fn anthropic_and_responses_keep_text_arrays_on_the_wire() -> Result<(), TestError> {
    let body = json!({"messages":[{"role":"user","content":[
        {"type":"text","text":"old"},{"type":"text","text":"new"}
    ]}]});
    for protocol in [RigProtocol::Anthropic, RigProtocol::Responses] {
        let result = dispatch(protocol, &body, WireBudget::default()).await?;
        assert_eq!(result.error, None);
        let received = result.body.ok_or("no HTTP body")?;
        assert_eq!(serde_json::from_slice::<Value>(&received)?, body);
        if protocol == RigProtocol::Responses {
            assert_eq!(received, serde_json::to_vec_pretty(&body)?);
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_wire_budget_measures_the_normalized_body() -> Result<(), TestError> {
    let texts: Vec<_> = (0..100).map(|index| format!("part {index}")).collect();
    let parts: Vec<_> = texts
        .iter()
        .map(|text| json!({"type":"text","text":text}))
        .collect();
    let body = json!({"model":"m","max_tokens":4,"messages":[{"role":"user","content":parts}]});
    let mut expected = body.clone();
    expected["messages"][0]["content"] = texts.join("\n\n").into();
    let tokens =
        codex_utils_string::approx_tokens_from_byte_count(serde_json::to_vec(&expected)?.len());
    let window = tokens + 4;
    assert!(
        codex_utils_string::approx_tokens_from_byte_count(serde_json::to_vec(&body)?.len())
            > window
    );
    let result = dispatch(
        RigProtocol::Chat,
        &body,
        WireBudget {
            context_window_tokens: Some(i64::try_from(window)?),
            output_tokens: Some(4),
            vision_floor_tokens: 0,
        },
    )
    .await?;
    assert_eq!(result.error, None);
    assert_eq!(result.attempts, 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&result.body.ok_or("no HTTP body")?)?,
        expected
    );
    Ok(())
}

#[tokio::test]
async fn oversized_normalized_chat_text_is_rejected_before_http() -> Result<(), TestError> {
    let body =
        json!({"messages":[{"role":"user","content":[{"type":"text","text":"x".repeat(2048)}]}]});
    let result = dispatch(
        RigProtocol::Chat,
        &body,
        WireBudget {
            context_window_tokens: Some(64),
            output_tokens: Some(4),
            vision_floor_tokens: 0,
        },
    )
    .await?;
    assert!(matches!(
        result.budget_error,
        Some(codex_api::ApiError::ContextWindowExceeded)
    ));
    assert_eq!((result.body, result.attempts), (None, 0));
    Ok(())
}

#[tokio::test]
async fn post_compact_consecutive_users_cross_rig_and_http_as_one_ordered_string()
-> Result<(), TestError> {
    let texts = [
        "<environment_context>fixture</environment_context>",
        "Reply with exactly: live-ok",
        "Compaction summary: the previous liveness exchange is complete; follow the next user request.",
        "Reply with exactly: post-compact-ok",
    ];
    let input = texts
        .iter()
        .map(|text| {
            serde_json::from_value(json!({
                "type":"message","role":"user","content":[{"type":"input_text","text":text}]
            }))
        })
        .collect::<Result<Vec<codex_protocol::models::ResponseItem>, _>>()?;
    let request = codex_api::ResponsesApiRequest {
        model: "test-model".into(),
        stream: true,
        service_tier: None,
        input,
        tools: None,
        tool_choice: "none".into(),
        parallel_tool_calls: false,
        max_output_tokens: Some(32),
        reasoning: None,
        store: false,
        stream_options: None,
        include: Vec::new(),
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    };
    let (completion, _) = crate::convert_request::responses_request_to_completion_request(
        &request,
        RigProtocol::Chat,
        "fixture",
    )?;
    let client = super::RigHttpClient {
        inner: reqwest_rig::Client::builder().no_proxy().build()?,
        protocol: RigProtocol::Chat,
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let (_stop, stopped) = oneshot::channel();
    let server = tokio::spawn(capture_one(
        listener,
        stopped,
        Arc::clone(&attempts),
        CAPTURE_RESPONSE,
    ));
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer dummy-wire-test"),
    );
    let model = crate::client::build_chat_model(
        "test-model",
        &format!("http://{address}/v1"),
        &headers,
        client,
    )?;
    let mut stream = model.stream(completion)?;
    // The server returns an intentional HTTP error after capturing the request;
    // no simulated LLM answer can make the outbound assertion pass.
    let rejected = tokio::time::timeout(DEADLINE, async {
        while let Some(item) = stream.next().await {
            if item.is_err() {
                return true;
            }
        }
        false
    })
    .await?;
    assert!(rejected, "capture-only endpoint must reject inference");
    let received = tokio::time::timeout(DEADLINE, server)
        .await???
        .ok_or("no SDK HTTP request")?;
    let body: Value = serde_json::from_slice(&received)?;
    assert_eq!(
        body["messages"],
        json!([{"role":"user","content":texts.join("\n\n")}])
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    Ok(())
}
