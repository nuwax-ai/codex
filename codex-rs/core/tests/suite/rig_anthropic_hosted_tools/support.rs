//! Shared fixtures for the Anthropic-bridge core suites: the scripted mock
//! gateway (SSE frames + request capture) and the wire-block builders the
//! hosted-tools, identity, and credential-instance suites reuse.

use anyhow::Context;
use anyhow::Result;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

pub(super) fn anthropic_provider(base_url: &str) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "rig-anthropic".into(),
        base_url: Some(format!("{base_url}/v1")),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        experimental_bridge: None,
        provider_id: Some("anthropic-hosted-test".into()),
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api: WireApi::Anthropic,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        include_internal_metadata: false,
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

/// Sequence server for `/v1/messages`: each POST receives the next Anthropic
/// SSE body, and every request body is captured for deep comparison.
pub(super) struct AnthropicSequence {
    num_calls: AtomicUsize,
    bodies: Vec<String>,
    pub(super) requests: Mutex<Vec<Value>>,
}

pub(super) struct AnthropicResponder(Arc<AnthropicSequence>);

impl Respond for AnthropicResponder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        self.0.respond(request)
    }
}

impl AnthropicSequence {
    pub(super) fn record(&self, body: Value) {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(body);
    }

    /// Every captured request body, in arrival order.
    pub(super) fn captured(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Respond for AnthropicSequence {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        if let Ok(body) = serde_json::from_slice::<Value>(&request.body) {
            self.record(body);
        }
        let call = self.num_calls.fetch_add(1, Ordering::SeqCst);
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_raw(
                self.bodies
                    .get(call)
                    .expect("missing Anthropic response for call")
                    .clone(),
                "text/event-stream",
            )
    }
}

pub(super) async fn mount_anthropic_sequence(
    server: &MockServer,
    bodies: Vec<String>,
) -> Arc<AnthropicSequence> {
    let responder = Arc::new(AnthropicSequence {
        num_calls: AtomicUsize::new(0),
        bodies,
        requests: Mutex::new(Vec::new()),
    });
    Mock::given(method("POST"))
        .respond_with(AnthropicResponder(responder.clone()))
        .mount(server)
        .await;
    responder
}

/// Consume the complete request before signalling that the response can stall.
/// Both header and body limits keep malformed fixtures from growing the buffer.
pub(super) async fn read_anthropic_request(socket: &mut TcpStream) -> Result<Value> {
    const MAX_HEADER_BYTES: usize = 64 * 1024;
    const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 4096];
        let read = socket
            .read(&mut chunk)
            .await
            .context("read request headers")?;
        anyhow::ensure!(read != 0, "socket closed before complete request headers");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            anyhow::ensure!(
                index + 4 <= MAX_HEADER_BYTES,
                "request headers exceed limit"
            );
            break index + 4;
        }
        anyhow::ensure!(
            data.len() < MAX_HEADER_BYTES,
            "request headers exceed limit"
        );
    };
    let headers = std::str::from_utf8(&data[..header_end]).context("request header UTF-8")?;
    anyhow::ensure!(
        headers.lines().next() == Some("POST /v1/messages HTTP/1.1"),
        "unexpected Anthropic request line: {headers}"
    );
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim())
        })
        .context("request Content-Length")?
        .parse::<usize>()
        .context("parse request Content-Length")?;
    anyhow::ensure!(length <= MAX_BODY_BYTES, "request body exceeds limit");
    while data.len() < header_end + length {
        let mut chunk = [0; 4096];
        let remaining = (header_end + length - data.len()).min(chunk.len());
        let read = socket
            .read(&mut chunk[..remaining])
            .await
            .context("read request body")?;
        anyhow::ensure!(read != 0, "socket closed before complete request body");
        data.extend_from_slice(&chunk[..read]);
    }
    serde_json::from_slice(&data[header_end..header_end + length])
        .context("parse Anthropic request JSON")
}

pub(super) fn message_start(id: &str) -> String {
    format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"{id}\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{{\"input_tokens\":4,\"output_tokens\":0}}}}}}\n\n",
        ),
        id = id,
    )
}

pub(super) fn block_stop(index: usize) -> String {
    format!(
        "event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{index}}}\n\n"
    )
}

pub(super) fn message_delta(stop: &str) -> String {
    format!(
        concat!(
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{stop}\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":6}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        stop = stop,
    )
}

pub(super) fn server_tool_use_block(index: usize, id: &str, query: &str) -> String {
    format!(
        concat!(
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"server_tool_use\",\"id\":\"{id}\",\"name\":\"web_search\",\"input\":{{}}}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{{\\\"query\\\":\\\"{query}\\\"}}\"}}}}\n\n",
        ),
        index = index,
        id = id,
        query = query,
    )
}

pub(super) fn tool_use_block(index: usize, id: &str) -> String {
    format!(
        concat!(
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"tool_use\",\"id\":\"{id}\",\"name\":\"update_plan\",\"input\":{{}}}}}}\n\n",
        ),
        index = index,
        id = id,
    )
}

pub(super) fn search_result_block(index: usize, tool_use_id: &str, encrypted: &str) -> String {
    format!(
        concat!(
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"{tool_use_id}\",\"content\":[{{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"{encrypted}\"}}]}}}}\n\n",
        ),
        index = index,
        tool_use_id = tool_use_id,
        encrypted = encrypted,
    )
}

pub(super) fn cited_text_block(index: usize, text: &str) -> String {
    format!(
        concat!(
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"text\",\"text\":\"\",\"citations\":[{{\"type\":\"search_result_location\",\"cited_text\":\"finding\",\"source\":\"https://example.com\",\"title\":\"Example\",\"search_result_index\":0,\"start_block_index\":1,\"end_block_index\":2}}]}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n",
        ),
        index = index,
        text = text,
    )
}

pub(super) fn plain_text_block(index: usize, text: &str) -> String {
    format!(
        concat!(
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n",
            "event: content_block_stop\n",
            "data: {{\"type\":\"content_block_stop\",\"index\":{index}}}\n\n",
        ),
        index = index,
        text = text,
    )
}

pub(super) fn search_replay_blocks(id: &str, query: &str, encrypted: &str) -> Vec<Value> {
    vec![
        json!({"type":"server_tool_use","id":id,"name":"web_search","input":{"query":query}}),
        json!({"type":"web_search_tool_result","tool_use_id":id,"content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":encrypted}]}),
    ]
}

pub(super) fn cited_replay_block(text: &str) -> Value {
    json!({"type":"text","text":text,"citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]})
}

pub(super) fn expected_follow_up_messages(
    prefix: &Value,
    content: Vec<Value>,
    prompt: &str,
) -> Value {
    let mut messages = prefix.as_array().expect("initial request messages").clone();
    messages.push(json!({"role":"assistant","content":content}));
    messages.push(json!({"role":"user","content":[{"type":"text","text":prompt}]}));
    Value::Array(messages)
}
