#![cfg(not(target_family = "wasm"))]

use bytes::Bytes;
use codex_api::RetryConfig;
use pretty_assertions::assert_eq;
use rig_core::http_client::HttpClientExt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

type TestError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug)]
enum ClientRetryPolicy {
    ProviderControlled,
    ReqwestDefault,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2_refused_stream_retries_obey_provider_budget_and_capture_every_attempt()
-> Result<(), TestError> {
    let deadline = Duration::from_secs(/*secs*/ 5);
    for (client_policy, max_attempts, retry_transport, expected) in [
        (ClientRetryPolicy::ProviderControlled, 0, true, (1, 1)),
        (ClientRetryPolicy::ProviderControlled, 2, false, (1, 1)),
        (ClientRetryPolicy::ProviderControlled, 1, true, (2, 2)),
        (ClientRetryPolicy::ReqwestDefault, 0, true, (3, 1)),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let wire_attempts = Arc::new(AtomicUsize::new(/*v*/ 0));
        let server_attempts = Arc::clone(&wire_attempts);
        let (stop, mut stopped) = watch::channel(/*init*/ false);
        let capture = Arc::new(Mutex::new(crate::FinalRequestCapture::default()));
        let builder = match client_policy {
            ClientRetryPolicy::ProviderControlled => super::http_client_builder(),
            // retry::Builder::default() is private. A fresh Client builder restores
            // reqwest's actual default: two extra protocol-NACK retries, no budget.
            ClientRetryPolicy::ReqwestDefault => reqwest_rig::Client::builder(),
        };
        let client = crate::transport::RigHttpClient {
            inner: builder.http2_prior_knowledge().no_proxy().build()?,
            retry: Some(RetryConfig {
                max_attempts,
                base_delay: Duration::ZERO,
                retry_429: false,
                retry_5xx: false,
                retry_transport,
            }),
            protocol: crate::RigProtocol::Responses,
            final_request_recorder: Some(Arc::clone(&capture)),
            ..Default::default()
        };
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri(format!("http://{address}/responses"))
            .version(http::Version::HTTP_2)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Bytes::from_static(
                b"{\"model\":\"h2-retry-test\",\"stream\":true}",
            ))?;

        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    accepted = tokio::time::timeout(deadline, listener.accept()) => {
                        let (socket, _) = accepted??;
                        let attempts = Arc::clone(&server_attempts);
                        let mut connection_stop = stopped.clone();
                        connections.spawn(async move {
                            let mut connection = tokio::select! {
                                biased;
                                _ = connection_stop.changed() => return Ok::<(), TestError>(()),
                                result = tokio::time::timeout(deadline, h2::server::handshake(socket)) => result??,
                            };
                            loop {
                                // Poll accept again after each reset to flush RST_STREAM.
                                // A single connection may carry every HTTP retry.
                                tokio::select! {
                                    biased;
                                    _ = connection_stop.changed() => return Ok(()),
                                    accepted = tokio::time::timeout(deadline, connection.accept()) => {
                                        let Some((request, mut response)) = accepted?.transpose()? else {
                                            return Ok(());
                                        };
                                        assert_eq!(request.uri().path(), "/responses");
                                        attempts.fetch_add(/*val*/ 1, Ordering::SeqCst);
                                        response.send_reset(h2::Reason::REFUSED_STREAM);
                                    }
                                }
                            }
                        });
                    }
                }
            }
            while let Some(result) = connections.join_next().await {
                result??;
            }
            Ok::<(), TestError>(())
        });

        let outcome = tokio::time::timeout(deadline, client.send_streaming(request)).await;
        // Stop based on the completed client operation, never a predicted wire count.
        stop.send(/*value*/ true)?;
        tokio::time::timeout(deadline, server).await???;
        assert!(outcome?.is_err(), "every H2 request was refused");
        let capture = capture.lock().expect("request capture");
        assert_eq!(
            (wire_attempts.load(Ordering::SeqCst), capture.requests.len()),
            expected,
            "client_policy={client_policy:?}, max_attempts={max_attempts}, retry_transport={retry_transport}"
        );
    }
    Ok(())
}
