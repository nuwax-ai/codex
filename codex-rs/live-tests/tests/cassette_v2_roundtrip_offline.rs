//! Offline v2 rig-event cassette round-trip: record single attempts against
//! local SSE servers through the ACTUAL recorder, persist through the real
//! `save_rig_event_fixture` serialization/layout, reload from disk, and
//! replay through the current conversion — then require the complete event
//! lists to match. Only two generated values are normalized: synthesized
//! `rigseg_…` item ids and the v2 reasoning envelope's endpoint source
//! (live builds it from the provider, replay uses the cassette label).
//!
//! Everything else — reasoning text and signatures, native blocks, tool
//! call ids, usage counters, terminal errors — must survive the disk
//! round-trip byte-for-byte. Fixtures are recorded into a per-test tempdir;
//! checked-in v1 fixtures are untouched.

use anyhow::Context;
use anyhow::Result;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_api::ResponsesApiRequest;
use codex_live_tests::load_rig_event_fixture_at;
use codex_live_tests::save_rig_event_fixture_at;
use codex_live_tests::shared_auth;
use codex_live_tests::vendor_provider;
use codex_protocol::protocol::ReportedResponseUsage;
use codex_protocol::protocol::ReportedUsageCounters;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::replay_fixture_events;
use codex_rust_rig_bridge::stream_via_rig_with_recording;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const VENDOR: &str = "roundtrip";

fn bridge_request(tools: Option<Value>) -> Result<ResponsesApiRequest> {
    let mut request = ResponsesApiRequest {
        model: "roundtrip-model".into(),
        input: vec![
            serde_json::from_value::<codex_protocol::models::ResponseItem>(json!({
                "type":"message", "role":"user",
                "content":[{"type":"input_text","text":"offline roundtrip"}]
            }))
            .context("fixture user item")?,
        ],
        tools: None,
        tool_choice: "auto".into(),
        parallel_tool_calls: true,
        max_output_tokens: None,
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
    if let Some(tools) = tools {
        let raw: Arc<serde_json::value::RawValue> =
            Arc::from(serde_json::value::to_raw_value(&tools).context("tools raw value")?);
        request.tools = Some(raw.into());
    }
    Ok(request)
}

/// One-shot local SSE gateway: serve `payload` to the single request.
async fn serve_sse(listener: &tokio::net::TcpListener, payload: &str) -> Result<()> {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    let (mut socket, _) = listener
        .accept()
        .await
        .context("accept roundtrip request")?;
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 8192];
        let read = socket.read(&mut chunk).await.context("read request")?;
        assert_ne!(read, 0, "client closed before the request completed");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&data[..header_end]).context("request headers UTF-8")?;
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .context("request content length")?;
    while data.len() < header_end + length {
        let mut chunk = [0u8; 8192];
        let read = socket.read(&mut chunk).await.context("read request body")?;
        assert_ne!(read, 0);
        data.extend_from_slice(&chunk[..read]);
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}

fn normalize_segment_id(value: &str) -> Option<String> {
    let rest = value.strip_prefix("rigseg_")?;
    let (response, ordinal) = rest.rsplit_once('_')?;
    if ordinal.is_empty() || !ordinal.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if response.is_empty() || !response.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("rigseg_normalized_{ordinal}"))
}

fn normalize_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            if let Some(normalized) = normalize_segment_id(text) {
                *text = normalized;
                return;
            }
            if let Some(encoded) = text.strip_prefix("codex-rig-reasoning-v2:") {
                let Ok(mut inner) = serde_json::from_str::<Value>(encoded) else {
                    return;
                };
                if let Some(Value::String(source)) = inner.get_mut("source") {
                    *source = "normalized-source".into();
                }
                if let Ok(reencoded) = serde_json::to_string(&inner) {
                    *text = format!("codex-rig-reasoning-v2:{reencoded}");
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_value),
        Value::Object(map) => map.values_mut().for_each(normalize_value),
        _ => {}
    }
}

fn normalized_events(events: Vec<Result<ResponseEvent, ApiError>>) -> Result<Value> {
    let mapped: Vec<Value> = events
        .into_iter()
        .map(|event| match event {
            Ok(event) => {
                let mut value = serde_json::to_value(&event).context("serialize response event")?;
                normalize_value(&mut value);
                Ok(value)
            }
            Err(error) => Ok(json!({"stream_error": format!("{error:?}")})),
        })
        .collect::<Result<_>>()?;
    Ok(Value::Array(mapped))
}

struct Roundtrip {
    protocol: RigProtocol,
    sse: String,
    tools: Option<Value>,
    tag: &'static str,
}

/// Records one attempt through the live recorder and pipeline, saves,
/// reloads and replays it, then requires equality of the normalized event
/// lists. Returns both lists for scenario-specific asserts.
async fn roundtrip(scene: Roundtrip, root: &Path) -> Result<(Value, Value)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind roundtrip gateway")?;
    let address = listener.local_addr().context("roundtrip address")?;
    let payload = scene.sse.clone();
    let server = tokio::spawn(async move { serve_sse(&listener, &payload).await });
    let provider = vendor_provider(VENDOR, &format!("http://{address}/v1"));
    let request = bridge_request(scene.tools.clone())?;
    let recorder = Arc::new(std::sync::Mutex::new(
        codex_rust_rig_bridge::RigRecording::default(),
    ));
    let start = stream_via_rig_with_recording(
        &request,
        &provider,
        &shared_auth("offline-cassette-key"),
        http::HeaderMap::new(),
        scene.protocol,
        Duration::from_secs(10),
        Some(recorder.clone()),
    )
    .await;
    let (mut stream, _) = start.context("roundtrip stream starts")?;
    let mut live = Vec::new();
    while let Some(event) = stream.rx_event.recv().await {
        live.push(event);
    }
    server
        .await
        .context("server task")?
        .context("server served")?;

    let recording = recorder
        .lock()
        .map_err(|_| anyhow::anyhow!("recorder lock poisoned"))?
        .clone();
    assert!(
        !recording.events.is_empty(),
        "the recorder observed the rig events of the attempt"
    );
    let custom_tools = codex_rust_rig_bridge::extract_custom_tool_names(&request);
    let path = save_rig_event_fixture_at(root, VENDOR, scene.tag, &recording, &custom_tools)
        .context("persist v2 fixture")?;
    let fixture = load_rig_event_fixture_at(&path)
        .map_err(anyhow::Error::msg)
        .context("reload v2 fixture")?;
    assert!(
        fixture.rig_events.is_empty(),
        "a fresh recording must use the v2 items layout, not legacy rig_events"
    );
    let replayed = replay_fixture_events(&fixture);
    let mut replay_events: Vec<Result<ResponseEvent, ApiError>> =
        replayed.events.into_iter().map(Ok).collect();
    if let Some(error) = replayed.error {
        replay_events.push(Err(error));
    }
    let live = normalized_events(live)?;
    let replay = normalized_events(replay_events)?;
    Ok((live, replay))
}

fn chat_sse(frames: &[Value]) -> String {
    let mut body = String::new();
    for frame in frames {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    body + "data: [DONE]\n\n"
}

fn chat_frame(delta: Value, finish: Option<&str>) -> Value {
    let mut choice = json!({"index": 0, "delta": delta});
    choice["finish_reason"] = finish.map(Value::from).unwrap_or(Value::Null);
    json!({
        "id": "chatcmpl-roundtrip", "object": "chat.completion.chunk", "created": 1,
        "model": "roundtrip-model", "choices": [choice],
    })
}

fn chat_usage(usage: Value) -> Value {
    json!({
        "id": "chatcmpl-roundtrip", "object": "chat.completion.chunk", "created": 1,
        "model": "roundtrip-model", "choices": [], "usage": usage,
    })
}

fn anthropic_sse(blocks: Vec<Value>, stop: &str, terminal_usage: Value) -> String {
    let mut payload = String::from("event: message_start\ndata: ");
    payload.push_str(
        &json!({"type":"message_start","message":{"id":"msg-roundtrip","type":"message",
            "role":"assistant","content":[],"model":"roundtrip-model","stop_reason":null,
            "stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":0,
            "cache_read_input_tokens":70,"cache_creation_input_tokens":20}}})
        .to_string(),
    );
    payload.push_str("\n\n");
    for (index, block) in blocks.iter().enumerate() {
        payload.push_str(&format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({"type":"content_block_start","index":index,"content_block":block})
        ));
        if block["type"] == "text" {
            payload.push_str(&format!(
                "event: content_block_delta\ndata: {}\n\n",
                json!({"type":"content_block_delta","index":index,
                    "delta":{"type":"text_delta","text":block["text"]}})
            ));
        }
        payload.push_str(&format!(
            "event: content_block_stop\ndata: {}\n\n",
            json!({"type":"content_block_stop","index":index})
        ));
    }
    payload.push_str(&format!(
        "event: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
        json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},
            "usage":terminal_usage}),
        json!({"type":"message_stop"})
    ));
    payload
}

fn thinking_block() -> Value {
    json!({"type":"thinking","thinking":"visible thought","signature":"SIG_ROUNDTRIP"})
}

fn envelope_of(events: &Value) -> String {
    events
        .to_string()
        .split("codex-rig-reasoning-v2:")
        .nth(1)
        .and_then(|rest| {
            // Grab the envelope's JSON back out of the serialized event.
            let bytes = rest.as_bytes();
            let mut depth = 0usize;
            let mut end = None;
            for (index, byte) in bytes.iter().enumerate() {
                match byte {
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(index + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            end.map(|end| String::from_utf8_lossy(&bytes[..end]).into_owned())
        })
        .unwrap_or_default()
}

/// Chat: reasoning → text, with a late usage-only chunk carrying cache
/// counters after the finish frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_reasoning_text_late_usage_cache_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Chat,
            sse: chat_sse(&[
                chat_frame(
                    json!({"role":"assistant","reasoning_content":"planning"}),
                    None,
                ),
                chat_frame(json!({"content":"answer"}), Some("stop")),
                chat_usage(
                    json!({"prompt_tokens":23,"completion_tokens":7,"total_tokens":30,
                    "prompt_tokens_details":{"cached_tokens":128}}),
                ),
            ]),
            tools: None,
            tag: "chat-reasoning-text",
        },
        root.path(),
    )
    .await?;
    assert_eq!(
        live, replay,
        "the full event list survives the disk round-trip"
    );
    assert!(
        live.to_string().contains("SIG_ROUNDTRIP") || live.to_string().contains("planning"),
        "reasoning content is present: {live}"
    );
    assert!(
        live.to_string().contains("\"total_tokens\":30"),
        "late usage survives: {live}"
    );
    assert!(
        live.to_string().contains("\"cached_input_tokens\":128"),
        "cache counters survive: {live}"
    );
    Ok(())
}

/// Chat: reasoning → custom tool call; the fixture's custom-tool list must
/// restore the CustomToolCall identity and the provider's call id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_reasoning_custom_tool_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Chat,
            sse: chat_sse(&[
                chat_frame(
                    json!({"role":"assistant","reasoning_content":"planning"}),
                    None,
                ),
                chat_frame(
                    json!({"tool_calls":[{
                        "index":0,"id":"call_custom_rt","type":"function",
                        "function":{"name":"exec","arguments":"{\"input\":\"echo hi\"}"}
                    }]}),
                    None,
                ),
                chat_frame(json!({}), Some("tool_calls")),
                chat_usage(json!({"prompt_tokens":5,"completion_tokens":2,"total_tokens":7})),
            ]),
            tools: Some(json!([{"type":"custom","name":"exec"}])),
            tag: "chat-reasoning-tool",
        },
        root.path(),
    )
    .await?;
    assert_eq!(
        live, replay,
        "the full event list survives the disk round-trip"
    );
    let encoded = live.to_string();
    assert!(
        encoded.contains("call_custom_rt"),
        "provider tool id survives: {encoded}"
    );
    assert!(
        encoded.contains("echo hi"),
        "custom tool input survives: {encoded}"
    );
    Ok(())
}

/// Anthropic: signed thinking → text with cache usage; the v2 envelope's
/// native signature must survive the disk round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_signed_thinking_text_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Anthropic,
            sse: anthropic_sse(
                vec![thinking_block(), json!({"type":"text","text":"answer"})],
                "end_turn",
                json!({"output_tokens":6}),
            ),
            tools: None,
            tag: "anthropic-thinking-text",
        },
        root.path(),
    )
    .await?;
    assert_eq!(
        live, replay,
        "the full event list survives the disk round-trip"
    );
    for (label, events) in [("live", &live), ("replay", &replay)] {
        let envelope = envelope_of(events);
        assert!(
            envelope.contains("SIG_ROUNDTRIP"),
            "{label}: native signature survives: {envelope}"
        );
    }
    // Inclusive input (4 + 70 + 20) and cache counters both survive.
    assert!(
        live.to_string().contains("\"input_tokens\":94"),
        "inclusive input usage survives: {live}"
    );
    Ok(())
}

/// Anthropic: thinking → client tool call; the provider call id pairs the
/// result on the next request in both live and replay outputs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_thinking_tool_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Anthropic,
            sse: anthropic_sse(
                vec![
                    thinking_block(),
                    json!({"type":"tool_use","id":"call_tool_rt","name":"lookup","input":{}}),
                ],
                "tool_use",
                json!({"output_tokens":6}),
            ),
            tools: Some(json!([{"type":"function","name":"lookup",
                "parameters":{"type":"object","properties":{}}}])),
            tag: "anthropic-thinking-tool",
        },
        root.path(),
    )
    .await?;
    assert_eq!(
        live, replay,
        "the full event list survives the disk round-trip"
    );
    assert!(
        live.to_string().contains("call_tool_rt"),
        "provider tool id survives: {live}"
    );
    Ok(())
}

/// Anthropic: the output cap terminal (max_tokens) must surface as the
/// identical CapExhausted error, with wire-corrected usage, after the
/// round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cap_error_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Anthropic,
            sse: anthropic_sse(
                vec![json!({"type":"text","text":"partial cap answer"})],
                "max_tokens",
                json!({"output_tokens":64}),
            ),
            tools: None,
            tag: "anthropic-cap",
        },
        root.path(),
    )
    .await?;
    assert_eq!(live, replay, "the cap error survives the disk round-trip");
    let encoded = live.to_string();
    assert!(
        encoded.contains("CapExhausted"),
        "the terminal is the output cap: {encoded}"
    );
    assert!(
        encoded.contains("Output token limit reached"),
        "the error names the fix: {encoded}"
    );
    assert!(
        encoded.contains("output_tokens: Some(64)"),
        "the cap usage report survives: {encoded}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_cap_error_preserves_reported_usage_presence_roundtrips() -> Result<()> {
    let cases = [
        (
            "complete",
            json!({"prompt_tokens":23,"completion_tokens":7,"total_tokens":30,
                "prompt_tokens_details":{"cached_tokens":11},
                "completion_tokens_details":{"reasoning_tokens":3}}),
            ReportedUsageCounters {
                input_tokens: Some(23),
                cached_input_tokens: Some(11),
                output_tokens: Some(7),
                reasoning_output_tokens: Some(3),
                total_tokens: Some(30),
                ..Default::default()
            },
        ),
        (
            "zero",
            json!({"prompt_tokens":0,"completion_tokens":0,"total_tokens":0,
                "prompt_tokens_details":{"cached_tokens":0},
                "completion_tokens_details":{"reasoning_tokens":0}}),
            ReportedUsageCounters {
                input_tokens: Some(0),
                cached_input_tokens: Some(0),
                output_tokens: Some(0),
                reasoning_output_tokens: Some(0),
                total_tokens: Some(0),
                ..Default::default()
            },
        ),
        (
            "partial",
            json!({"completion_tokens":7}),
            ReportedUsageCounters {
                output_tokens: Some(7),
                ..Default::default()
            },
        ),
    ];
    for (tag, usage, counts) in cases {
        let root = tempfile::tempdir().expect("fixture root");
        let (live, replay) = roundtrip(
            Roundtrip {
                protocol: RigProtocol::Chat,
                sse: chat_sse(&[
                    chat_frame(json!({"content":"partial cap answer"}), None),
                    chat_frame(json!({}), Some("length")),
                    chat_usage(usage),
                ]),
                tools: None,
                tag,
            },
            root.path(),
        )
        .await?;
        let path = root
            .path()
            .join(VENDOR)
            .join(format!("rig-events-{tag}.json"));
        let terminal = load_rig_event_fixture_at(&path)
            .expect("reload persisted terminal")
            .terminal
            .expect("recorded terminal");
        let expected_report = ReportedResponseUsage::from_counters(counts);
        assert_eq!(terminal.wire_reported_usage, expected_report, "{tag}");
        let expected_error = ApiError::CapExhausted {
            message: "Output token limit reached; increase max_tokens before retrying".into(),
            response_id: terminal.response_id,
            reported_usage: expected_report,
        };
        assert_eq!(
            live.as_array().expect("live events").last(),
            Some(&json!({"stream_error": format!("{expected_error:?}")})),
            "{tag}: the live error retains only directly reported counters"
        );
        assert_eq!(
            live, replay,
            "{tag}: the complete cap error and streamed prefix survive"
        );
    }
    Ok(())
}

/// Chat: content_filter terminal must replay as the identical error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_content_filter_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Chat,
            sse: chat_sse(&[
                chat_frame(json!({"content":"partial"}), None),
                chat_frame(json!({}), Some("content_filter")),
            ]),
            tools: None,
            tag: "chat-filter",
        },
        root.path(),
    )
    .await?;
    assert_eq!(
        live, replay,
        "the filter error survives the disk round-trip"
    );
    assert!(
        live.to_string().contains("ContentFilter"),
        "the terminal is the content filter: {live}"
    );
    Ok(())
}

/// Anthropic: an unmapped stop reason must replay as the identical
/// stream error naming the reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_unknown_finish_roundtrips() -> Result<()> {
    let root = tempfile::tempdir().expect("fixture root");
    let (live, replay) = roundtrip(
        Roundtrip {
            protocol: RigProtocol::Anthropic,
            sse: anthropic_sse(
                vec![json!({"type":"text","text":"partial"})],
                "weird_finish_reason",
                json!({"output_tokens":2}),
            ),
            tools: None,
            tag: "anthropic-unknown",
        },
        root.path(),
    )
    .await?;
    assert_eq!(
        live, replay,
        "the unknown-finish error survives the disk round-trip"
    );
    let encoded = live.to_string();
    assert!(
        encoded.contains("Incomplete Rig response"),
        "the terminal names the incomplete response: {encoded}"
    );
    Ok(())
}
