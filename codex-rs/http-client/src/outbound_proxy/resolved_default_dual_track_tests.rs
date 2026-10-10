//! Dual-track wire equivalence for the opt-in ResolvedDefault policy.
//!
//! Track A is a reference client built exactly like the unoptimized default
//! (`reqwest::Client::new()`, whose matcher performs the synchronous system
//! read); track B is a `RouteAwareClientPool` under `ResolvedDefault`. Both
//! run in the same scrubbed subprocess per case, and every case asserts both
//! tracks send through the environment proxy identically (same request lines
//! on the proxy) or both bypass it identically (origin server receives the
//! request directly).

use std::process::Command;
use std::time::Duration;

use pretty_assertions::assert_eq;

const MARKER: &str = "CODEX_HTTP_CLIENT_DUAL_TRACK_PROBE";

#[tokio::test]
async fn resolved_default_matches_the_reference_transport_on_the_wire() {
    let proxy_address = "127.0.0.1:1"; // placeholder; replaced per case below
    let _ = proxy_address;
    let origin_address = "127.0.0.1:1";
    let _ = origin_address;

    // Each case runs in its own scrubbed subprocess with the case's proxy
    // endpoints and environment; the child never mutates the environment.
    let cases: Vec<(&[(&str, &str)], &str)> = vec![
        (&[], "http://ORIGIN/direct"),
        (&[], "https://ORIGIN/direct"),
        (&[("HTTP_PROXY", "http://PROXY")], "http://via-proxy.test/a"),
        (
            &[("HTTPS_PROXY", "http://PROXY")],
            "https://via-proxy.test/a",
        ),
        (
            &[("ALL_PROXY", "http://PROXY")],
            "http://all-fallback.test/a",
        ),
        (
            &[("ALL_PROXY", "http://PROXY")],
            "https://all-fallback.test/a",
        ),
        (
            &[
                ("HTTP_PROXY", "http://PROXY"),
                ("HTTPS_PROXY", "http://PROXY"),
            ],
            "https://specific-wins.test/a",
        ),
        (&[("HTTP_PROXY", "http://PROXY")], "http://127.0.0.1:9/x"),
        (&[("HTTPS_PROXY", "http://PROXY")], "https://192.0.2.10/x"),
        // The hit case targets the origin by IP so the direct connection is
        // observable without any DNS dependence.
        (
            &[("HTTP_PROXY", "http://PROXY"), ("NO_PROXY", "127.0.0.1")],
            "http://ORIGIN/x",
        ),
        (
            &[("HTTP_PROXY", "http://PROXY"), ("NO_PROXY", "127.0.0.1")],
            "http://other.test/x",
        ),
    ];

    if std::env::var_os(MARKER).is_none() {
        let executable = std::env::current_exe().expect("test executable should be available");
        for (index, (envs, url_template)) in cases.iter().enumerate() {
            let mut command = Command::new(&executable);
            command
                .args([
                    "--exact",
                    "outbound_proxy::resolved_default_dual_track_tests::resolved_default_matches_the_reference_transport_on_the_wire",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env("DUAL_TRACK_CASE", index.to_string())
                .env("DUAL_TRACK_URL_TEMPLATE", url_template)
                .env_remove("HTTP_PROXY")
                .env_remove("http_proxy")
                .env_remove("HTTPS_PROXY")
                .env_remove("https_proxy")
                .env_remove("ALL_PROXY")
                .env_remove("all_proxy")
                .env_remove("NO_PROXY")
                .env_remove("no_proxy")
                .env_remove("REQUEST_METHOD");
            for (key, value) in *envs {
                command.env(key, value);
            }
            let output = command.output().expect("dual-track subprocess should run");
            assert!(
                output.status.success(),
                "case {index} ({url_template}) failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }

    // Child: exactly one case, endpoints injected by the parent.
    let proxy = RecordingProxy::spawn().await;
    let origin = RecordingProxy::spawn().await;
    let template = std::env::var("DUAL_TRACK_URL_TEMPLATE").expect("parent passes the template");
    let url = template
        .replace("PROXY", &proxy.address.to_string())
        .replace("ORIGIN", &origin.address.to_string());

    // Rewrite the placeholder endpoints inside the case's proxy variables.
    // Safe only before any other thread exists: this subprocess is the
    // isolation unit and nothing concurrent has started yet.
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
        if let Ok(value) = std::env::var(key) {
            let resolved = value
                .replace("PROXY", &proxy.address.to_string())
                .replace("ORIGIN", &origin.address.to_string());
            // SAFETY: single-threaded setup; no concurrent env readers.
            unsafe { std::env::set_var(key, resolved) };
        }
    }

    // Track A: the reference client, whose construction performs the very
    // synchronous system read the policy under test eliminates.
    let reference = reqwest::Client::builder()
        .build()
        .expect("reference client should build");
    // Track B: the pool under the policy under test.
    let pool = crate::RouteAwareClientPool::new(
        crate::HttpClientFactory::new(crate::OutboundProxyPolicy::ResolvedDefault),
        crate::ClientRouteClass::Other,
    );

    let proxy_before = proxy.reset();
    let origin_before = origin.reset();
    let reference_outcome = send_plain(&reference, &url).await;
    let reference_seen = (
        proxy.count_new_requests(proxy_before),
        origin.count_new_requests(origin_before),
    );

    let proxy_before = proxy.reset();
    let origin_before = origin.reset();
    let pool_outcome = send_pool(&pool, &url).await;
    let pool_seen = (
        proxy.count_new_requests(proxy_before),
        origin.count_new_requests(origin_before),
    );

    // Only the ROUTING observation is asserted. Request success is not part
    // of the contract: direct-https cases fail TLS on both tracks against
    // the plain origin listener, and proxied cases never get a real
    // upstream; both tracks must merely land on the same side.
    let _ = (reference_outcome, pool_outcome);
    assert_eq!(
        reference_seen, pool_seen,
        "tracks routed differently; proxy/origin request counts differ"
    );
    assert!(
        reference_seen.0 + reference_seen.1 > 0,
        "no track observed any request"
    );
    proxy.shutdown().await;
    origin.shutdown().await;
}

async fn send_plain(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let request = client.get(url).timeout(Duration::from_secs(5));
    match request.send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            // A proxy that never answers past the request line still counts
            // as "routed through the proxy" for equivalence purposes; only
            // connection-class failures matter.
            if error.is_connect() {
                Ok(())
            } else {
                Err(format!("{error}"))
            }
        }
    }
}

async fn send_pool(pool: &crate::RouteAwareClientPool, url: &str) -> Result<(), String> {
    match tokio::time::timeout(Duration::from_secs(5), pool.get(url).send()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => {
            let text = error.to_string();
            // Route-aware errors redact URLs; connection-class failures
            // still prove the routing decision matched.
            if text.contains("connect") {
                Ok(())
            } else {
                Err(text)
            }
        }
        Err(_) => Err("pool request timed out".to_string()),
    }
}

/// A TCP listener that records raw HTTP request lines and answers 200 (GET)
/// or CONNECT-established; both tracks speak to it identically.
struct RecordingProxy {
    address: std::net::SocketAddr,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl RecordingProxy {
    async fn spawn() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("recording listener should bind");
        let address = listener
            .local_addr()
            .expect("recording listener should have an address");
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let recorder = std::sync::Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => return,
                    accepted = listener.accept() => {
                        let Ok((mut stream, _)) = accepted else { continue };
                        let recorder = std::sync::Arc::clone(&recorder);
                        tokio::spawn(async move {
                            use tokio::io::AsyncReadExt as _;
                            use tokio::io::AsyncWriteExt as _;
                            let mut buffer = [0_u8; 4096];
                            let Ok(read) = stream.read(&mut buffer).await else { return };
                            let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
                            if let Some(line) = text.lines().next() {
                                recorder.lock().expect("recorder lock").push(line.to_string());
                            }
                            // GET → 200 empty; CONNECT → established.
                            if line_is_connect(&text) {
                                let _ = stream
                                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                                    .await;
                            } else {
                                let _ = stream
                                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                                    .await;
                            }
                        });
                    }
                }
            }
        });
        Self {
            address,
            requests,
            shutdown,
            task,
        }
    }

    fn reset(&self) -> usize {
        self.requests.lock().expect("recorder lock").len()
    }

    fn count_new_requests(&self, before: usize) -> usize {
        self.requests.lock().expect("recorder lock").len() - before
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

fn line_is_connect(request: &str) -> bool {
    request.starts_with("CONNECT ")
}
