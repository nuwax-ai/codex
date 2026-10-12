//! Cross-version proxy parity between the workspace's two reqwest majors.
//!
//! The fork ships two live HTTP stacks: the codex core stack on the
//! workspace `reqwest` 0.12 line (native paths, `codex-http-client`) and
//! the rig bridge's transport on reqwest 0.13. Both read proxy
//! configuration from the environment by default, so a user's env or CGI
//! context must route BOTH stacks identically. Each case runs in its own
//! scrubbed subprocess; every case asserts the 0.12 reference client, the
//! 0.13 bridge-style client, and the ResolvedDefault pool land on the same
//! side of the recording proxy (or the origin).

use std::process::Command;
use std::time::Duration;

use pretty_assertions::assert_eq;

const MARKER: &str = "CODEX_RIG_PROXY_VERSION_PARITY_PROBE";

#[tokio::test]
async fn reqwest_012_and_013_route_env_proxies_identically() {
    let cases: &[(&[(&str, &str)], &str)] = &[
        (&[("HTTP_PROXY", "http://PROXY")], "http://env-hit.test/a"),
        (
            &[("HTTPS_PROXY", "http://PROXY")],
            "https://env-https.test/a",
        ),
        (
            &[("HTTP_PROXY", "http://PROXY"), ("NO_PROXY", "localhost")],
            "http://localhost:ORIGINPORT/bypassed",
        ),
        // A suffix without a domain boundary must not bypass the proxy.
        (
            &[("HTTP_PROXY", "http://PROXY"), ("NO_PROXY", "host")],
            "http://localhost:ORIGINPORT/still-proxied",
        ),
        (
            &[("HTTP_PROXY", "http://PROXY"), ("NO_PROXY", "other.test")],
            "http://env-hit.test/still-proxied",
        ),
        // CGI disables the proxy on both majors: the request must go
        // DIRECT to a reachable origin.
        (
            &[("HTTP_PROXY", "http://PROXY"), ("REQUEST_METHOD", "GET")],
            "http://ORIGIN/cgi-direct",
        ),
        // A redirect must keep using the proxy on both majors.
        (&[("HTTP_PROXY", "http://PROXY")], "http://REDIRECTOR/start"),
    ];

    if std::env::var_os(MARKER).is_none() {
        let executable = std::env::current_exe().expect("test executable should be available");
        for (index, (envs, url_template)) in cases.iter().enumerate() {
            let mut command = Command::new(&executable);
            command
                .args([
                    "--exact",
                    "proxy_version_parity_tests::reqwest_012_and_013_route_env_proxies_identically",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env("PARITY_CASE", index.to_string())
                .env("PARITY_URL", url_template)
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
            let output = command.output().expect("parity subprocess should run");
            assert!(
                output.status.success(),
                "case {index} ({url_template}) failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }

    // Child: one case with substituted endpoints.
    let proxy = CountingListener::spawn().await;
    let redirector = CountingListener::spawn().await;
    let origin = CountingListener::spawn().await;
    let template = std::env::var("PARITY_URL").expect("parent passes the url");
    let url = template
        .replace("ORIGINPORT", &origin.address.port().to_string())
        .replace("REDIRECTOR", &redirector.address.to_string())
        .replace("PROXY", &proxy.address.to_string())
        .replace("ORIGIN", &origin.address.to_string());
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
        if let Ok(value) = std::env::var(key) {
            let resolved = value
                .replace("PROXY", &proxy.address.to_string())
                .replace("ORIGIN", &origin.address.to_string());
            // SAFETY: single-threaded setup; no concurrent env readers.
            unsafe { std::env::set_var(key, resolved) };
        }
    }

    // The 0.12 reference: exactly how the codex core stack's default client
    // is built.
    let client012 = reqwest012::Client::builder()
        .build()
        .expect("0.12 reference client should build");
    // The 0.13 bridge-style client: same options `client::http_client`
    // passes (retry-never), no proxy customization.
    let client013 = reqwest_rig::Client::builder()
        .retry(reqwest_rig::retry::never())
        .build()
        .expect("0.13 bridge-style client should build");
    // The pool under the policy under test (0.12 transport under the hood).
    let pool = codex_http_client::RouteAwareClientPool::new(
        codex_http_client::HttpClientFactory::new(
            codex_http_client::OutboundProxyPolicy::ResolvedDefault,
        ),
        codex_http_client::ClientRouteClass::Other,
    );

    let before = (proxy.reset(), redirector.reset(), origin.reset());
    let outcome012 = send012(&client012, &url).await;
    let seen012 = counts(&proxy, &redirector, &origin, before);
    let proxy_requests012 = proxy.requests_since(before.0);

    let before = (proxy.reset(), redirector.reset(), origin.reset());
    let outcome013 = send013(&client013, &url).await;
    let seen013 = counts(&proxy, &redirector, &origin, before);
    let proxy_requests013 = proxy.requests_since(before.0);

    let before = (proxy.reset(), redirector.reset(), origin.reset());
    let outcome_pool = send_pool(&pool, &url).await;
    let seen_pool = counts(&proxy, &redirector, &origin, before);
    let proxy_requests_pool = proxy.requests_since(before.0);

    assert_eq!(
        seen012, seen013,
        "reqwest 0.12 and 0.13 routed the case differently"
    );
    assert_eq!(
        seen012, seen_pool,
        "the ResolvedDefault pool routed the case differently"
    );
    assert_eq!(
        proxy_requests012, proxy_requests013,
        "reqwest 0.12 and 0.13 sent different proxy request paths"
    );
    assert_eq!(
        proxy_requests012, proxy_requests_pool,
        "the ResolvedDefault pool sent different proxy request paths"
    );
    if template.contains("REDIRECTOR") {
        assert_eq!(
            (outcome012, outcome013, outcome_pool),
            (Ok(200), Ok(200), Ok(200)),
            "the redirect chain must complete on every stack"
        );
        assert_eq!(seen012, (2, 0, 0), "both redirect hops use the proxy");
        assert_eq!(
            proxy_requests012,
            vec![
                format!("GET http://{}/start HTTP/1.1", redirector.address),
                format!("GET http://{}/final HTTP/1.1", redirector.address),
            ],
            "the proxy sees the initial and final paths exactly once"
        );
    } else if template.ends_with("/bypassed") || template.ends_with("/cgi-direct") {
        assert_eq!(seen012, (0, 0, 1), "the request reaches the direct origin");
        assert_eq!(
            (outcome012, outcome013, outcome_pool),
            (Ok(200), Ok(200), Ok(200)),
            "direct routing must complete on every stack"
        );
    } else {
        assert_eq!(seen012, (1, 0, 0), "the request uses the proxy");
    }
    proxy.shutdown().await;
    redirector.shutdown().await;
    origin.shutdown().await;
}

fn counts(
    proxy: &CountingListener,
    redirector: &CountingListener,
    origin: &CountingListener,
    before: (usize, usize, usize),
) -> (usize, usize, usize) {
    (
        proxy.count_new_requests(before.0),
        redirector.count_new_requests(before.1),
        origin.count_new_requests(before.2),
    )
}

async fn send012(client: &reqwest012::Client, url: &str) -> Result<u16, String> {
    let request = client.get(url).timeout(Duration::from_secs(5));
    request
        .send()
        .await
        .map(|response| response.status().as_u16())
        .map_err(|error| format!("{error:#}"))
}

async fn send013(client: &reqwest_rig::Client, url: &str) -> Result<u16, String> {
    let request = client.get(url).timeout(Duration::from_secs(5));
    request
        .send()
        .await
        .map(|response| response.status().as_u16())
        .map_err(|error| format!("{error:#}"))
}

async fn send_pool(
    pool: &codex_http_client::RouteAwareClientPool,
    url: &str,
) -> Result<u16, String> {
    match tokio::time::timeout(Duration::from_secs(5), pool.get(url).send()).await {
        Ok(Ok(response)) => Ok(response.status().as_u16()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("pool request timed out".to_string()),
    }
}

/// Counts request lines per connection and answers 200; a listener whose
/// port is addressed as `REDIRECTOR` answers a redirect to `/final` on the
/// first request.
struct CountingListener {
    address: std::net::SocketAddr,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl CountingListener {
    async fn spawn() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("counting listener should bind");
        let address = listener
            .local_addr()
            .expect("counting listener should have an address");
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
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
                            // The redirector answers /start with one hop to
                            // /final; everything else is a plain 200.
                            let is_start = text.contains(" /start?")
                                || text.ends_with(" /start HTTP/1.1")
                                || text.contains("/start HTTP/1.1");
                            if is_start {
                                let _ = stream
                                    .write_all(b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
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

    fn requests_since(&self, before: usize) -> Vec<String> {
        self.requests.lock().expect("recorder lock")[before..].to_vec()
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}
