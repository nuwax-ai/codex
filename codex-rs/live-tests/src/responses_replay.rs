//! Responses cassette replay through the actual request and HTTP path.

use std::time::Duration;

use codex_api::ResponseEvent;
use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
use http::HeaderMap;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::time::timeout;

use crate::LiveConfig;
use crate::drain_stream;
use crate::load_responses_sse_fixture;
use crate::shared_auth;
use crate::vendor_provider;

const REPLAY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HEADER_BYTES: usize = 64 * 1024;

pub(crate) async fn replay_turn(
    cfg: &LiveConfig,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    let original = serde_json::to_vec(request).expect("serialize original bridge input");

    // The only permitted change is clearing bridge-owned reasoning envelopes.
    // Serialize the typed copy to also check raw tool schemas, key order and
    // the full reasoning_text payload, rather than a lossy JSON round trip.
    let mut expected = request.clone();
    for item in &mut expected.input {
        if let ResponseItem::Reasoning {
            encrypted_content, ..
        } = item
            && encrypted_content
                .as_deref()
                .is_some_and(codex_rust_rig_bridge::is_replay_envelope)
        {
            *encrypted_content = None;
        }
    }
    let expected = serde_json::to_vec(&expected).expect("serialize expected wire request");
    let sse = load_responses_sse_fixture(&cfg.vendor, tag)
        .unwrap_or_else(|error| panic!("[cassette-responses] {error}"));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Responses replay server");
    let address = listener.local_addr().expect("Responses replay address");
    let provider = vendor_provider(&cfg.vendor, &format!("http://{address}/v1"));
    let replay = async {
        let server = serve_recording(&listener, &sse, expected.len());
        let client = async {
            let stream = codex_rust_rig_bridge::stream_via_rig(
                request,
                &provider,
                &shared_auth("offline-cassette-key"),
                HeaderMap::new(),
                codex_rust_rig_bridge::RigProtocol::Responses,
                REPLAY_TIMEOUT,
            )
            .await
            .expect("start Responses replay through Rig");
            drain_stream(stream, &cfg.vendor, tag).await
        };
        tokio::join!(server, client)
    };
    let (wire_body, events) = timeout(REPLAY_TIMEOUT, replay)
        .await
        .expect("Responses loopback replay completed within timeout");
    assert_eq!(
        wire_body, expected,
        "{}/{tag}: HTTP body must preserve every field except the replay envelope",
        cfg.vendor
    );
    assert_eq!(
        serde_json::to_vec(request).expect("serialize unchanged bridge input"),
        original,
        "Responses replay must not rewrite caller history"
    );
    events
}

async fn serve_recording(listener: &TcpListener, sse: &str, expected_length: usize) -> Vec<u8> {
    let (mut socket, _) = listener.accept().await.expect("accept Responses replay");
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 8192];
        let read = socket.read(&mut chunk).await.expect("read replay headers");
        assert_ne!(read, 0, "client closed before request headers");
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            assert!(index < MAX_HEADER_BYTES, "replay request headers too large");
            break index + 4;
        }
        assert!(
            data.len() < MAX_HEADER_BYTES,
            "replay request headers too large"
        );
    };
    let headers = std::str::from_utf8(&data[..header_end]).expect("UTF-8 replay headers");
    assert_eq!(headers.lines().next(), Some("POST /v1/responses HTTP/1.1"));
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().expect("replay content length"))
        })
        .expect("replay request content length");
    assert_eq!(length, expected_length, "unexpected replay request size");
    while data.len() < header_end + length {
        let mut chunk = [0; 8192];
        let read = socket.read(&mut chunk).await.expect("read replay body");
        assert_ne!(read, 0, "client closed before complete request body");
        data.extend_from_slice(&chunk[..read]);
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
        sse.len()
    );
    socket
        .write_all(response.as_bytes())
        .await
        .expect("write recorded Responses SSE");
    data[header_end..header_end + length].to_vec()
}
