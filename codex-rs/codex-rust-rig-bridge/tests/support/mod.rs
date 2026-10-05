use codex_api::AuthProvider;
use codex_api::Provider;
use codex_api::ResponsesApiRequest;
use codex_api::RetryConfig;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ResponseItem;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

pub fn request(items: Vec<Value>) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: "review-model".into(),
        instructions: "system instructions".into(),
        input: items
            .into_iter()
            .map(|item| serde_json::from_value::<ResponseItem>(item).expect("fixture item"))
            .collect(),
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
    }
}

pub fn user() -> Value {
    json!({"type":"message", "role":"user", "content":[{"type":"input_text","text":"hello"}]})
}
pub fn set_tools(request: &mut ResponsesApiRequest, tools: Value) {
    let raw: Arc<serde_json::value::RawValue> =
        Arc::from(serde_json::value::to_raw_value(&tools).expect("tools"));
    request.tools = Some(raw.into());
}

pub struct DummyAuth;
impl AuthProvider for DummyAuth {
    fn immutable_credential_headers(&self) -> Option<http::HeaderMap> {
        Some(self.to_auth_headers())
    }

    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        let mut value = http::HeaderValue::from_static("Bearer local-test-dummy");
        value.set_sensitive(true);
        headers.insert(http::header::AUTHORIZATION, value.clone());
        headers.insert("x-gateway-auth", value);
    }
}

/// Binds synthetic replay fixtures to the immutable credentials actually sent
/// by DummyAuth. Captured-event tests retain the source their real turn emitted.
pub fn source_for_dummy_auth(provider: &Provider, protocol: RigProtocol) -> String {
    let mut actual_headers = provider.headers.clone();
    actual_headers.extend(DummyAuth.immutable_credential_headers().unwrap());
    let mut scoped_headers = http::HeaderMap::new();
    for (name, value) in &actual_headers {
        if !codex_api::is_benign_request_header(name.as_str()) {
            scoped_headers.append(name.clone(), value.clone());
        }
    }
    let domain = codex_api::credential_instance_identity(provider, scoped_headers)
        .unwrap()
        .expect("bounded immutable test credentials");
    let auth_identity = serde_json::to_string(&("credentialInstance", domain)).unwrap();
    codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        provider,
        protocol,
        "review-model",
        Some(&auth_identity),
    )
    .expect("source identity")
}

pub const IMAGE: &str = "data:image/png;base64,aGVsbG8=";
pub const CHAT_SSE: &str = "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"server-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"server-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1,\"total_tokens\":5}}\n\ndata: [DONE]\n\n";
pub const ANTHROPIC_SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"server-model\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
/// Wire-shape Responses stream (data-only frames, mirroring CHAT_SSE): one
/// assistant message and a completed terminal with usage. Richer reasoning /
/// tool flows have dedicated fixtures in the responses wire tests.
pub const RESPONSES_SSE: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-test\",\"model\":\"server-model\"}}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"id\":\"msg_1\"}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"ok\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}],\"id\":\"msg_1\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-test\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1,\"total_tokens\":5}}}\n\n",
);

pub async fn capture(
    request: &ResponsesApiRequest,
    protocol: RigProtocol,
) -> (Value, Vec<codex_api::ResponseEvent>, Option<String>) {
    capture_with_auth(request, protocol, Arc::new(DummyAuth)).await
}

pub async fn capture_with_auth(
    request: &ResponsesApiRequest,
    protocol: RigProtocol,
    auth: SharedAuthProvider,
) -> (Value, Vec<codex_api::ResponseEvent>, Option<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let payload = match protocol {
            RigProtocol::Responses => RESPONSES_SSE,
            RigProtocol::Chat => CHAT_SSE,
            RigProtocol::Anthropic => ANTHROPIC_SSE,
        };
        serve_payload(&listener, payload).await
    });
    let provider = Provider {
        name: "local test".into(),
        base_url: format!("http://{address}/v1?existing=a%26b"),
        query_params: Some(
            [("api-version".into(), "version with & spaces".into())]
                .into_iter()
                .collect(),
        ),
        headers: http::HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(3),
        max_output_tokens: None,
        hosted_results_replay: None,
    };
    let mut stream = stream_via_rig(
        request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        protocol,
        Duration::from_secs(3),
    )
    .await
    .expect("start stream");
    let request_id = stream.upstream_request_id.clone();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }
    let wire = tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server timeout")
        .expect("server task");
    (wire, events, request_id)
}

/// Capture one actual SDK request and reply with scripted provider SSE.
pub async fn serve_payload(listener: &tokio::net::TcpListener, payload: &str) -> Value {
    serve_payload_with_headers(listener, payload, &[("x-request-id", "req-local")]).await
}

/// Reply with provider-specific diagnostic headers as well as scripted SSE.
pub async fn serve_payload_with_headers(
    listener: &tokio::net::TcpListener,
    payload: &str,
    response_headers: &[(&str, &str)],
) -> Value {
    let (mut socket, _) = listener.accept().await.expect("accept");
    let request = read_request(&mut socket).await;
    let response_headers: String = response_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n{response_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    socket
        .write_all(response.as_bytes())
        .await
        .expect("response");
    request
}

/// Read a full HTTP request, retaining its original body before JSON parsing.
pub async fn read_request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut data = Vec::new();
    let split = loop {
        let mut chunk = [0; 8192];
        let read = socket.read(&mut chunk).await.expect("read");
        assert_ne!(read, 0);
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(data[..split].to_vec()).expect("headers");
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().expect("length"))
        })
        .expect("content length");
    while data.len() < split + length {
        let mut chunk = [0; 8192];
        let read = socket.read(&mut chunk).await.expect("body");
        assert_ne!(read, 0);
        data.extend_from_slice(&chunk[..read]);
    }
    let body_raw = std::str::from_utf8(&data[split..split + length]).expect("request UTF-8");
    let body: Value = serde_json::from_str(body_raw).expect("request JSON");
    json!({"request_line":headers.lines().next(), "headers":headers, "body":body, "body_raw":body_raw})
}

/// A loopback server that answers each request in sequence with its payload
/// and records every request body (the parsed body JSON, not the wrapper).
pub async fn sequence_server(
    payloads: Vec<String>,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<Vec<serde_json::Value>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for payload in payloads {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let request = crate::support::read_request(&mut socket).await;
            bodies.push(request["body"].clone());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: req\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await.expect("write");
        }
        bodies
    });
    (address, server)
}
