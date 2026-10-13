//! Offline verification of the typed `NUWAX_*` environment startup mode in
//! the live binary harness: a local scripted SSE server stands in for the
//! provider, and the REAL `codex-exec` binary must go
//! environment → CLI seeds → rig → wire with the credential referenced (never
//! persisted), the configured model and output cap on every request, and a
//! genuine tool round trip.
//!
//! No live vendor, no credentials, no network beyond loopback. The scene's
//! own capture evidence (url/model/cap per attempt) and the harness's
//! source/bin receipt apply unchanged. On success the typed-mode wrapper
//! additionally proves the credential and the ephemeral provider definition
//! never reached the child's config, rollouts or working files.
//!
//! ```text
//! cargo build -p codex-exec --bin codex-exec
//! cargo nextest run -p codex-live-tests --test nuwax_env_offline --no-capture
//! ```

use anyhow::Context;
use anyhow::Result;
use codex_live_tests::LiveConfig;
use codex_live_tests::codex_exec_binary;
use codex_live_tests::run_nuwax_env_marker_turn;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

const API_KEY: &str = "offline-env-key";
const MODEL: &str = "offline-env-model";
const OUTPUT_CAP: u64 = 2048;

fn chat_chunk(choice: Value) -> Value {
    json!({
        "id": "chatcmpl-env-offline", "object": "chat.completion.chunk", "created": 1,
        "model": MODEL, "choices": [choice],
    })
}

fn usage_chunk() -> Value {
    json!({
        "id": "chatcmpl-env-offline", "object": "chat.completion.chunk", "created": 1,
        "model": MODEL, "choices": [],
        "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6},
    })
}

fn chat_sse(frames: Vec<Value>) -> String {
    let mut payload = frames
        .into_iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>();
    payload.push_str("data: [DONE]\n\n");
    payload
}

/// Reads one full HTTP/1.1 request (headers + Content-Length body).
async fn read_request(socket: &mut TcpStream) -> Result<(String, Value)> {
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 8192];
        let read = socket.read(&mut chunk).await?;
        anyhow::ensure!(read != 0, "connection closed before request headers");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
        })
        .flatten()
        .context("content length header")?;
    while data.len() < header_end + length {
        let mut chunk = [0u8; 8192];
        let read = socket.read(&mut chunk).await?;
        anyhow::ensure!(read != 0, "connection closed before request body");
        data.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&data[header_end..header_end + length])
        .context("request body json")?;
    Ok((headers, body))
}

/// Extracts the scene's unforgeable marker from the marker prompt
/// (`…真实执行命令 \`echo <marker>\`…`) so the scripted tool call echoes the
/// exact marker the scenario asserts on.
fn marker_from_prompt(body: &Value) -> Result<String> {
    let prompt = body["messages"]
        .as_array()
        .context("messages array")?
        .iter()
        .filter(|&message| message["role"] == "user")
        .filter_map(|message| match &message["content"] {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => parts
                .iter()
                .find_map(|part| part["text"].as_str().map(str::to_string)),
            _ => None,
        })
        .find(|text| text.contains("echo "))
        .context("marker prompt with `echo`")?;
    let marker = prompt
        .split("echo ")
        .nth(1)
        .context("text after echo")?
        .split('`')
        .next()
        .context("marker terminated by backtick")?
        .trim()
        .to_string();
    anyhow::ensure!(!marker.is_empty(), "empty marker extracted");
    Ok(marker)
}

/// The shell-family tool the binary actually advertises, with its native
/// argument spelling (`exec_command` takes `cmd`, `shell` takes `command`).
fn advertised_shell_tool(body: &Value) -> Result<&str> {
    body["tools"]
        .as_array()
        .context("tools array")?
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap_or_default())
        .find(|name| matches!(*name, "exec_command" | "shell"))
        .context("no shell-family tool advertised")
}

fn tool_call_arguments(name: &str, marker: &str) -> String {
    match name {
        "shell" => json!({"command": ["echo", marker]}).to_string(),
        _ => json!({"cmd": format!("echo {marker}")}).to_string(),
    }
}

fn assert_request_shape(headers: &str, body: &Value) -> Result<()> {
    assert!(
        headers.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "unexpected request line: {headers}"
    );
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {API_KEY}"))),
        "the env-referenced credential must reach the provider: {headers}"
    );
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["max_tokens"], OUTPUT_CAP);
    Ok(())
}

async fn respond_sse(socket: &mut TcpStream, payload: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn nuwax_env_startup_reaches_rig_with_tool_round_trip_and_no_leak() -> Result<()> {
    // Fail fast with the harness's own build instructions when the binary is
    // missing; the manifest receipt inside the scene checks source identity.
    codex_exec_binary()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    // Two exchanges: (1) the scripted tool call for the scene's marker, (2)
    // the final answer after the real binary executed the tool. Request
    // shapes are asserted server-side, where the raw bytes are still present.
    let server = tokio::spawn(async move {
        let mut marker: Option<String> = None;
        for index in 0..2 {
            let (mut socket, _) = listener.accept().await?;
            let (headers, body) = read_request(&mut socket).await?;
            assert_request_shape(&headers, &body)?;
            if index == 0 {
                let extracted = marker_from_prompt(&body)?;
                let tool = advertised_shell_tool(&body)?;
                let payload = chat_sse(vec![
                    chat_chunk(
                        json!({"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}),
                    ),
                    chat_chunk(json!({"index": 0, "delta": {"tool_calls": [
                        {"index": 0, "id": "call-env-offline", "type": "function",
                         "function": {"name": tool, "arguments": tool_call_arguments(tool, &extracted)}}
                    ]}, "finish_reason": null})),
                    chat_chunk(json!({"index": 0, "delta": {}, "finish_reason": "tool_calls"})),
                    usage_chunk(),
                ]);
                respond_sse(&mut socket, &payload).await?;
                marker = Some(extracted);
            } else {
                let marker = marker
                    .as_deref()
                    .context("marker captured before tool result")?;
                let tool_result = body["messages"]
                    .as_array()
                    .context("tool result messages")?
                    .iter()
                    .find(|message| {
                        message["role"] == "tool" && message["tool_call_id"] == "call-env-offline"
                    })
                    .context("the executed call's tool result")?;
                let content = tool_result["content"]
                    .as_str()
                    .context("tool result text")?;
                anyhow::ensure!(
                    content.contains(marker),
                    "the executed call's output must return to the provider: {tool_result}"
                );
                let payload = chat_sse(vec![
                    chat_chunk(
                        json!({"index": 0, "delta": {"role": "assistant", "content": marker}, "finish_reason": null}),
                    ),
                    chat_chunk(json!({"index": 0, "delta": {}, "finish_reason": "stop"})),
                    usage_chunk(),
                ]);
                respond_sse(&mut socket, &payload).await?;
            }
        }
        anyhow::Ok(marker.expect("marker captured on the first exchange"))
    });

    let cfg = LiveConfig {
        vendor: "nuwax-env-offline".to_string(),
        api_key: API_KEY.to_string(),
        base_url: format!("http://{addr}/v1"),
        anthropic_base_url: None,
        responses_base_url: None,
        model: MODEL.to_string(),
    };
    run_nuwax_env_marker_turn(
        /*protocol*/ "chat-env-offline",
        &cfg,
        &cfg.base_url,
        /*wire_api*/ "chat",
        Some(OUTPUT_CAP),
        /*extra_config*/ "",
        Some("Dispatching chat stream via rig"),
        /*http_attempt_budget*/ None,
    )
    .await?;

    let marker = tokio::time::timeout(Duration::from_secs(10), server).await???;
    assert!(!marker.is_empty(), "marker extracted server-side");
    Ok(())
}
