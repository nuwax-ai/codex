//! The provider's `stream_idle_timeout` governs two timers on the Chat and
//! Anthropic wires: the bridge's own event-idle timer in the pump, and rig's
//! frame-idle backstop inside the driver.

use super::*;

type TestError = Box<dyn std::error::Error + Send + Sync>;

const RIG_DEFAULT: Duration = rig_core::driver::STREAM_IDLE_TIMEOUT;

/// rig's backstop is never shorter than the configured wait — one configured
/// above rig's five-minute default must not be cut short — and never shorter
/// than its own default either, so a shorter configured wait still expires on
/// the bridge's timer and reports the same timeout as before.
#[test]
fn rig_stall_backstop_never_shortens_the_configured_idle_timeout() {
    assert_eq!(rig_stream_idle_timeout(Duration::ZERO), RIG_DEFAULT);
    assert_eq!(
        rig_stream_idle_timeout(Duration::from_millis(1)),
        RIG_DEFAULT
    );
    assert_eq!(rig_stream_idle_timeout(RIG_DEFAULT), RIG_DEFAULT);
    let longer = RIG_DEFAULT + Duration::from_secs(600);
    assert_eq!(
        rig_stream_idle_timeout(longer),
        longer,
        "a wait configured above rig's default must reach rig whole"
    );
}

/// The builders hand rig's driver the adjusted value. rig keeps the timer
/// private, so its `Debug` output is the only public read.
#[test]
fn the_configured_idle_timeout_reaches_rigs_driver() {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer dummy-idle-test"),
    );
    let longer = RIG_DEFAULT + Duration::from_secs(600);
    let chat = build_chat_model(
        "test-model",
        "http://127.0.0.1:1/v1",
        &headers,
        crate::transport::RigHttpClient::default(),
        longer,
    )
    .expect("chat model");
    let anthropic = build_anthropic_model(
        "test-model",
        "http://127.0.0.1:1/v1",
        &headers,
        crate::transport::RigHttpClient::default(),
        false,
        &[],
        longer,
    )
    .expect("anthropic model");
    for (wire, model) in [
        ("chat", format!("{chat:?}")),
        ("anthropic", format!("{anthropic:?}")),
    ] {
        // The rendered model embeds the whole vendor catalog: report the
        // timer alone, never the model.
        let reported = model
            .split_once("idle_timeout: ")
            .map(|(_, timer)| timer.split(')').next().unwrap_or(timer));
        assert!(
            model.contains(&format!("idle_timeout: Some({longer:?})")),
            "{wire} must carry the configured wait into rig's driver; rig reported: {reported:?}"
        );
    }
}

/// Reads one request head and exactly its `content-length` body, so the reply
/// is not written while the client is still sending.
async fn drain_request(socket: &mut tokio::net::TcpStream) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(head_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
        let body_len = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if request.len() >= head_end + 4 + body_len {
            return Ok(());
        }
    }
}

/// A reply that sends one frame and then falls silent fails as rig's
/// `Stalled` once the timeout handed to the driver has passed. Left on rig's
/// fixed five-minute default it would hang here until the test's deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_reply_stalls_after_the_timeout_given_to_rigs_driver() -> Result<(), TestError> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let frame = "data: {\"id\":\"chatcmpl-stall\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        drain_request(&mut socket).await?;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await?;
        socket.write_all(frame.as_bytes()).await?;
        // The connection stays open and says nothing: a provider thinking
        // past its keep-alive, or one that forgot the rest of the reply.
        std::future::pending::<()>().await;
        Ok::<(), std::io::Error>(())
    });

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer dummy-stall-test"),
    );
    let built = build_chat_model(
        "test-model",
        &format!("http://{address}/v1"),
        &headers,
        crate::transport::RigHttpClient {
            inner: reqwest_rig::Client::builder().no_proxy().build()?,
            protocol: crate::RigProtocol::Chat,
            ..Default::default()
        },
        Duration::from_secs(1),
    )?;
    // The builders clamp to rig's default, and no test waits five minutes:
    // hand the driver a short timer directly to observe the forwarding.
    let rig_idle = Duration::from_millis(300);
    let mut stream = model_via_bridge(built.wire, built.transport, rig_idle)
        .stream(rig_core::completion::CompletionRequest::new("say hi"))?;
    let mut failure = None;
    let stalled_in_time = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(item) = stream.next().await {
            if let Err(error) = item {
                failure = Some(error);
            }
        }
    })
    .await;
    server.abort();
    stalled_in_time?;

    match failure {
        Some(rig_core::error::ProviderError::Stalled(idle)) => assert_eq!(idle, rig_idle),
        other => panic!("a silent reply must stall: {other:?}"),
    }
    Ok(())
}
