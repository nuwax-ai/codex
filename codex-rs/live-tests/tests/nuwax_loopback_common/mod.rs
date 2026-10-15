//! Shared helpers for the offline `NUWAX_*` loopback tests: a scripted Chat
//! SSE server, byte-level request parsing and the request-shape contract.
//! Server-side connection/request counts are the authority that a refused
//! dispatch never reached the wire.

use anyhow::Context;
use anyhow::Result;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub const API_KEY: &str = "offline-env-key";
pub const MODEL: &str = "offline-env-model";
pub const OUTPUT_CAP: u64 = 2048;

pub fn chat_chunk(choice: Value) -> Value {
    json!({"id": "chatcmpl-offline", "object": "chat.completion.chunk",
        "created": 0, "model": MODEL, "choices": [choice]})
}

pub fn usage_chunk() -> Value {
    json!({"id": "chatcmpl-offline", "object": "chat.completion.chunk",
        "created": 0, "model": MODEL, "choices": [],
        "usage": {"prompt_tokens": 21, "completion_tokens": 5, "total_tokens": 26}})
}

pub fn chat_sse(frames: Vec<Value>) -> String {
    let mut payload = String::new();
    for frame in frames {
        payload.push_str("data: ");
        payload.push_str(&frame.to_string());
        payload.push_str("\n\n");
    }
    payload.push_str("data: [DONE]\n\n");
    payload
}

/// Reads one HTTP request (headers + Content-Length body) from the socket.
pub async fn read_request(socket: &mut TcpStream) -> Result<(String, Value)> {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = socket.read(&mut byte).await?;
        anyhow::ensure!(read == 1, "connection closed before header terminator");
        raw.push(byte[0]);
        if raw.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let headers = String::from_utf8_lossy(&raw).into_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .context("content-length header")?;
    let mut body = vec![0u8; length];
    socket.read_exact(&mut body).await?;
    Ok((headers, serde_json::from_slice(&body)?))
}

pub fn marker_from_prompt(body: &Value) -> Result<String> {
    let prompt = body["messages"]
        .as_array()
        .context("messages array")?
        .iter()
        .filter(|&message| message["role"] == "user")
        .flat_map(|message| match &message["content"] {
            Value::String(text) => vec![text.clone()],
            // The user turn carries several text parts (environment context
            // first, the actual prompt after); scan every part.
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
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
pub fn advertised_shell_tool(body: &Value) -> Result<&str> {
    body["tools"]
        .as_array()
        .context("tools array")?
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap_or_default())
        .find(|name| matches!(*name, "exec_command" | "shell"))
        .context("no shell-family tool advertised")
}

pub fn tool_call_arguments(name: &str, marker: &str) -> String {
    match name {
        "shell" => json!({"command": ["echo", marker]}).to_string(),
        _ => json!({"cmd": format!("echo {marker}")}).to_string(),
    }
}

pub fn assert_request_shape(headers: &str, body: &Value) -> Result<()> {
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

pub async fn respond_sse(socket: &mut TcpStream, payload: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}

/// The tool-call SSE frames for the first exchange of a marker round trip.
pub fn tool_call_frames(tool: &str, marker: &str) -> String {
    chat_sse(vec![
        chat_chunk(
            json!({"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}),
        ),
        chat_chunk(json!({"index": 0, "delta": {"tool_calls": [
            {"index": 0, "id": "call-env-offline", "type": "function",
             "function": {"name": tool, "arguments": tool_call_arguments(tool, marker)}}
        ]}, "finish_reason": null})),
        chat_chunk(json!({"index": 0, "delta": {}, "finish_reason": "tool_calls"})),
        usage_chunk(),
    ])
}

/// The final-answer SSE frames echoing `answer` back.
pub fn final_answer_frames(answer: &str) -> String {
    chat_sse(vec![
        chat_chunk(
            json!({"index": 0, "delta": {"role": "assistant", "content": answer}, "finish_reason": null}),
        ),
        chat_chunk(json!({"index": 0, "delta": {}, "finish_reason": "stop"})),
        usage_chunk(),
    ])
}

/// Finds the newest retained artifacts directory for `vendor` under the
/// repository's `logs/live-<vendor>/` tree.
pub fn newest_artifacts_dir(vendor: &str) -> Result<std::path::PathBuf> {
    let root = codex_live_tests::repo_root()
        .context("repository root")?
        .join("logs")
        .join(format!("live-{vendor}"));
    let newest = std::fs::read_dir(&root)
        .with_context(|| format!("artifacts root {}", root.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let metadata = entry.metadata().ok()?;
            Some((metadata, entry.path()))
        })
        .filter(|(metadata, _)| metadata.is_dir())
        .max_by_key(|(metadata, _)| {
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        })
        .map(|(_, path)| path)
        .context("a retained artifacts directory")?;
    Ok(newest)
}
