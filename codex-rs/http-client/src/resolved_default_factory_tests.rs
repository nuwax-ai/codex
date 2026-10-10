//! Public factory routing across redirect hops and reused client destinations.

use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::ClientRouteClass;
use crate::HttpClientFactory;
use crate::OutboundProxyPolicy;

const PROBE_CASE: &str = "CODEX_HTTP_PUBLIC_FACTORY_REDIRECT_PROBE";
const ORIGIN_URL: &str = "CODEX_HTTP_PUBLIC_FACTORY_ORIGIN_URL";
const PROXY_START: &str = "http://requires-proxy.test/start";
const PROXY_FINAL: &str = "http://requires-proxy.test/final";

#[tokio::test]
async fn public_factory_resolves_redirects_and_reused_destinations() {
    if let Ok(case) = std::env::var(PROBE_CASE) {
        let origin = std::env::var(ORIGIN_URL).expect("parent supplies origin URL");
        let origin_final = origin.replace("/start", "/final");
        let requests = match case.as_str() {
            "direct_to_proxy" => vec![(origin, "proxy")],
            "proxy_to_direct" => vec![(PROXY_START.to_string(), "origin")],
            "reuse_direct_client" => vec![(origin, "origin"), (PROXY_FINAL.to_string(), "proxy")],
            "reuse_proxy_client" => {
                vec![(PROXY_START.to_string(), "proxy"), (origin_final, "origin")]
            }
            _ => panic!("unknown public factory probe case"),
        };
        let factory = HttpClientFactory::new(OutboundProxyPolicy::ResolvedDefault);
        let client = factory
            .build_client(&requests[0].0, ClientRouteClass::Api)
            .expect("public factory should build the client");
        for (url, expected_body) in requests {
            let response = client
                .get(&url)
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .expect("request should reach its selected route");
            assert_eq!(
                response.text().await.expect("response body should read"),
                expected_body
            );
        }
        return;
    }

    for case in [
        "direct_to_proxy",
        "proxy_to_direct",
        "reuse_direct_client",
        "reuse_proxy_client",
    ] {
        let origin_response = if case == "direct_to_proxy" {
            format!(
                "HTTP/1.1 302 Found\r\nLocation: {PROXY_FINAL}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
        } else {
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\norigin".to_string()
        };
        let origin = RecordingServer::spawn(origin_response).await;
        let origin_url = format!("http://{}/start", origin.address);
        let proxy_response = if case == "proxy_to_direct" {
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                origin.address
            )
        } else {
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nproxy".to_string()
        };
        let proxy = RecordingServer::spawn(proxy_response).await;
        let proxy_url = format!("http://{}", proxy.address);
        let output = tokio::task::spawn_blocking(move || {
            let mut command =
                Command::new(std::env::current_exe().expect("test executable should exist"));
            command.args([
                "--exact",
                "client_builder::resolved_default_factory_tests::public_factory_resolves_redirects_and_reused_destinations",
                "--nocapture",
                "--format=terse",
            ]);
            for key in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "no_proxy",
                "REQUEST_METHOD",
                "CODEX_CA_CERTIFICATE",
                "SSL_CERT_FILE",
                "SSL_CERT_DIR",
            ] {
                command.env_remove(key);
            }
            command
                .env(PROBE_CASE, case)
                .env(ORIGIN_URL, origin_url)
                .env("HTTP_PROXY", proxy_url)
                .env("NO_PROXY", "127.0.0.1")
                .output()
                .expect("public factory probe should run")
        })
        .await
        .expect("public factory probe should join");
        let actual = (origin.finish().await, proxy.finish().await);
        assert!(
            output.status.success(),
            "case {case} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = match case {
            "direct_to_proxy" | "reuse_direct_client" => (
                vec!["GET /start HTTP/1.1".to_string()],
                vec![format!("GET {PROXY_FINAL} HTTP/1.1")],
            ),
            "proxy_to_direct" | "reuse_proxy_client" => (
                vec!["GET /final HTTP/1.1".to_string()],
                vec![format!("GET {PROXY_START} HTTP/1.1")],
            ),
            _ => panic!("unknown public factory case"),
        };
        assert_eq!(
            actual, expected,
            "case {case} must select each route exactly once"
        );
    }
}

struct RecordingServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl RecordingServer {
    async fn spawn(response: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("recording listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let (stop, mut stop_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = tokio::select! {
                    _ = &mut stop_rx => break,
                    accepted = listener.accept() => accepted.expect("listener should accept"),
                };
                let request = tokio::time::timeout(Duration::from_secs(10), async {
                    let mut header = Vec::new();
                    let mut byte = [0_u8; 1];
                    while !header.ends_with(b"\r\n\r\n") {
                        if header.len() >= 8192 {
                            return Err(std::io::Error::other(
                                "request headers exceeded test limit",
                            ));
                        }
                        stream.read_exact(&mut byte).await?;
                        header.push(byte[0]);
                    }
                    Ok::<_, std::io::Error>(header)
                })
                .await;
                let Ok(Ok(request)) = request else {
                    continue;
                };
                let request = String::from_utf8(request).expect("request should be UTF-8");
                let first_line = request.lines().next().expect("request line should exist");
                recorded
                    .lock()
                    .expect("recorded requests lock")
                    .push(first_line.to_string());
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("listener should respond");
            }
        });
        Self {
            address,
            requests,
            stop,
            task,
        }
    }

    async fn finish(self) -> Vec<String> {
        let _ = self.stop.send(());
        self.task.await.expect("recording listener should finish");
        self.requests
            .lock()
            .expect("recorded requests lock")
            .clone()
    }
}
