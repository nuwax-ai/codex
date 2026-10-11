//! Real request-header dual-track for the opt-in ResolvedDefault policy.
//!
//! The routing dual-track (`resolved_default_dual_track_tests`) proves both
//! tracks pick the same SIDE (proxy vs origin). These tests go one level
//! deeper and compare the actual bytes each track puts on the proxy wire:
//! the request line, `Proxy-Authorization` (plain and percent-encoded
//! userinfo), CONNECT tunnel behavior including the TLS SNI the origin
//! observes, the IPv6 proxy literal, and a 407 challenge both tracks must
//! treat as a failure.
//!
//! Track A is a reference `reqwest::Client` built exactly like the
//! unoptimized default; track B is a `RouteAwareClientPool` under
//! `ResolvedDefault`. Every case runs in its own scrubbed subprocess.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::Engine as _;
use pretty_assertions::assert_eq;

const MARKER: &str = "CODEX_HTTP_CLIENT_AUTH_WIRE_PROBE";

#[tokio::test]
async fn resolved_default_proxy_auth_matches_the_reference_on_the_wire() {
    // (env additions with placeholders, url template, proxy mode)
    let cases: &[(&[(&str, &str)], &str, &str)] = &[
        // Plain userinfo: both tracks must send the identical
        // Proxy-Authorization on the absolute-URI request.
        (
            &[("HTTP_PROXY", "http://user:pass@PROXY")],
            "http://ORIGIN/path",
            "plain",
        ),
        // Percent-encoded userinfo decodes before base64 on both tracks.
        (
            &[("HTTP_PROXY", "http://us%20er:p%40ss@PROXY")],
            "http://ORIGIN/path",
            "plain",
        ),
        // HTTPS goes through an authenticated CONNECT tunnel; the TLS origin
        // records the SNI both tracks send.
        (
            &[("HTTPS_PROXY", "http://user:pass@PROXY")],
            "https://localhost:TLSORIGIN/x",
            "tunnel",
        ),
        // A 407 challenge fails both tracks; neither retries into success.
        (
            &[("HTTP_PROXY", "http://user:pass@PROXY")],
            "http://ORIGIN/path",
            "reject407",
        ),
        // An IPv6 proxy literal routes both tracks identically.
        (
            &[("HTTP_PROXY", "http://V6PROXY")],
            "http://ORIGIN/path",
            "plain",
        ),
    ];

    if std::env::var_os(MARKER).is_none() {
        let executable = std::env::current_exe().expect("test executable should be available");
        for (index, (envs, url_template, mode)) in cases.iter().enumerate() {
            let mut command = Command::new(&executable);
            command
                .args([
                    "--exact",
                    "outbound_proxy::resolved_default_auth_wire_tests::resolved_default_proxy_auth_matches_the_reference_on_the_wire",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env("AUTH_WIRE_CASE", index.to_string())
                .env("AUTH_WIRE_URL", url_template)
                .env("AUTH_WIRE_MODE", mode)
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
            let output = command.output().expect("auth-wire subprocess should run");
            assert!(
                output.status.success(),
                "case {index} ({url_template}, {mode}) failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }

    // Child: one case, endpoints injected by the parent.
    let mode = std::env::var("AUTH_WIRE_MODE").expect("parent passes the mode");
    let bind_v6 =
        mode == "plain" && std::env::var("HTTP_PROXY").is_ok_and(|value| value.contains("V6PROXY"));
    let proxy = HeaderRecordingProxy::spawn(&mode, /*bind_ipv6*/ bind_v6).await;
    let origin = PlainOrigin::spawn().await;
    let tls_origin = TlsSniOrigin::spawn().await;

    let template = std::env::var("AUTH_WIRE_URL").expect("parent passes the url");
    let url = template
        .replace("TLSORIGIN", &tls_origin.address.port().to_string())
        .replace("PROXY", &proxy.address.to_string())
        .replace("ORIGIN", &origin.address.to_string());
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
        if let Ok(value) = std::env::var(key) {
            let resolved = value
                .replace("V6PROXY", &proxy.address.to_string())
                .replace("PROXY", &proxy.address.to_string())
                .replace("ORIGIN", &origin.address.to_string());
            // SAFETY: single-threaded setup; no concurrent env readers.
            unsafe { std::env::set_var(key, resolved) };
        }
    }

    let reference = reqwest::Client::builder()
        .build()
        .expect("reference client should build");
    let pool = crate::RouteAwareClientPool::new(
        crate::HttpClientFactory::new(crate::OutboundProxyPolicy::ResolvedDefault),
        crate::ClientRouteClass::Other,
    );

    let reference_seen = proxy.reset();
    let reference_sni = tls_origin.reset();
    let reference_outcome = send_plain(&reference, &url).await;
    let reference_observation = (
        proxy.observation_since(reference_seen),
        tls_origin.sni_since(reference_sni),
    );

    let pool_seen = proxy.reset();
    let pool_sni = tls_origin.reset();
    let pool_outcome = send_pool(&pool, &url).await;
    let pool_observation = (
        proxy.observation_since(pool_seen),
        tls_origin.sni_since(pool_sni),
    );

    match mode.as_str() {
        "plain" => {
            assert_eq!(
                reference_observation, pool_observation,
                "plain mode: both tracks must emit identical proxy-side bytes"
            );
            let expected_auth = expected_basic_auth_for_current_env();
            let observed = reference_observation.0;
            assert_eq!(observed.len(), 1, "exactly one proxied request per track");
            let (line, auth, host) = &observed[0];
            let authority = host_addr(&url);
            assert!(
                line.starts_with(&format!("GET http://{authority}")),
                "absolute-URI request line through the proxy: {line}"
            );
            assert_eq!(auth, &expected_auth);
            assert!(host.is_some(), "Host header present: {observed:?}");
        }
        "tunnel" => {
            assert_eq!(
                reference_observation, pool_observation,
                "tunnel mode: CONNECT bytes and origin SNI must match"
            );
            let observed = reference_observation.0;
            assert_eq!(
                observed.len(),
                1,
                "exactly one CONNECT per track; url {url}, reference outcome was {reference_outcome:?}, pool outcome {pool_outcome:?}"
            );
            let (line, auth, _host) = &observed[0];
            assert!(line.starts_with("CONNECT "), "CONNECT request line: {line}");
            assert_eq!(
                auth.as_deref(),
                Some("Basic dXNlcjpwYXNz"),
                "authenticated CONNECT on both tracks"
            );
            let snis = &reference_observation.1;
            assert_eq!(
                snis.as_slice(),
                &[Some("localhost".to_string())],
                "the TLS origin saw the URL host as SNI"
            );
            // Both tracks fail certificate verification against the
            // self-signed origin; the SNI observation above already proves
            // the ClientHello reached the origin through the tunnel.
            assert!(reference_outcome.is_err());
            assert!(pool_outcome.is_err());
        }
        "reject407" => {
            assert_eq!(
                reference_observation, pool_observation,
                "407 mode: identical first requests"
            );
            // A 407 arrives as an HTTP response, not a transport error; both
            // tracks must surface the same status and neither may retry
            // into a different outcome.
            assert_eq!(
                reference_outcome, pool_outcome,
                "both tracks see the identical 407 outcome"
            );
            assert_eq!(reference_outcome, Ok(407));
            let observed = reference_observation.0;
            assert!(
                observed.len() <= 2,
                "no track may retry a 407 repeatedly: {observed:?}"
            );
        }
        other => panic!("unknown mode {other}"),
    }
    proxy.shutdown().await;
    origin.shutdown().await;
    tls_origin.shutdown().await;
}

/// The exact `Proxy-Authorization` value both tracks must derive from the
/// case's proxy userinfo, computed from the (already substituted) env.
fn expected_basic_auth_for_current_env() -> Option<String> {
    let proxy_url = std::env::var("HTTP_PROXY").expect("plain cases set HTTP_PROXY");
    let rest = proxy_url.trim_start_matches("http://");
    if rest.split('@').count() == 1 {
        return None;
    }
    let authority = rest.split('@').next_back().unwrap_or_default().to_string();
    let userinfo = rest
        .strip_suffix(&authority)
        .unwrap_or_default()
        .trim_end_matches('@');
    let decoded = percent_decode(userinfo);
    Some(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(decoded)
    ))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |byte: u8| {
                match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                }
                .map(|digit| digit as usize)
            };
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                output.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn host_addr(url: &str) -> String {
    url.trim_start_matches("http://")
        .split('/')
        .next()
        .expect("authority")
        .to_string()
}

/// `Ok(status)` when the exchange produced an HTTP response; `Err(text)`
/// for transport-level failures.
async fn send_plain(client: &reqwest::Client, url: &str) -> Result<u16, String> {
    let request = client.get(url).timeout(Duration::from_secs(5));
    match request.send().await {
        Ok(response) => Ok(response.status().as_u16()),
        Err(error) => Err(format!("{error:#}")),
    }
}

async fn send_pool(pool: &crate::RouteAwareClientPool, url: &str) -> Result<u16, String> {
    match tokio::time::timeout(Duration::from_secs(5), pool.get(url).send()).await {
        Ok(Ok(response)) => Ok(response.status().as_u16()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("pool request timed out".to_string()),
    }
}

/// A TCP proxy that records each connection's request line plus the
/// `Proxy-Authorization` and `Host` headers, and answers per `mode`.
struct HeaderRecordingProxy {
    address: std::net::SocketAddr,
    observations: Arc<std::sync::Mutex<Vec<ProxyObservation>>>,
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

type ProxyObservation = (String, Option<String>, Option<String>);

impl HeaderRecordingProxy {
    async fn spawn(mode: &str, bind_ipv6: bool) -> Self {
        let bind = if bind_ipv6 { "::1" } else { "127.0.0.1" };
        let listener = tokio::net::TcpListener::bind((bind, 0))
            .await
            .expect("proxy listener should bind");
        let address = listener
            .local_addr()
            .expect("proxy listener should have an address");
        let observations = Arc::new(std::sync::Mutex::new(Vec::<ProxyObservation>::new()));
        let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let mode = mode.to_string();
        let recorder = Arc::clone(&observations);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => return,
                    accepted = listener.accept() => {
                        let Ok((mut stream, _)) = accepted else { continue };
                        let recorder = Arc::clone(&recorder);
                        let mode = mode.clone();
                        tokio::spawn(async move {
                            let Some(head) = read_http_head(&mut stream).await else {
                                return;
                            };
                            let line = head.lines().next().unwrap_or_default().to_string();
                            let header = |name: &str| {
                                head.lines().skip(1).find_map(|line| {
                                    let (key, value) = line.split_once(':')?;
                                    key.trim()
                                        .eq_ignore_ascii_case(name)
                                        .then(|| value.trim().to_string())
                                })
                            };
                            recorder.lock().expect("recorder lock").push((
                                line.clone(),
                                header("Proxy-Authorization"),
                                header("Host"),
                            ));
                            match (mode.as_str(), line.starts_with("CONNECT")) {
                                ("reject407", _) => {
                                    use tokio::io::AsyncWriteExt as _;
                                    let _ = stream
                                        .write_all(
                                            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                        )
                                        .await;
                                }
                                (_, true) => {
                                    use tokio::io::AsyncWriteExt as _;
                                    let _ = stream
                                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                                        .await;
                                    // Real tunnel: relay to the CONNECT target.
                                    let target = line
                                        .split_whitespace()
                                        .nth(1)
                                        .unwrap_or_default()
                                        .to_string();
                                    if let Ok(mut upstream) = tokio::net::TcpStream::connect(&target).await {
                                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
                                    }
                                }
                                _ => {
                                    use tokio::io::AsyncWriteExt as _;
                                    let _ = stream
                                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                                        .await;
                                }
                            }
                        });
                    }
                }
            }
        });
        Self {
            address,
            observations,
            shutdown,
            task,
        }
    }

    fn reset(&self) -> usize {
        self.observations.lock().expect("recorder lock").len()
    }

    fn observation_since(&self, marker: usize) -> Vec<ProxyObservation> {
        self.observations.lock().expect("recorder lock")[marker..].to_vec()
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

/// Reads one full HTTP request head (up to the blank line).
async fn read_http_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt as _;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 16 * 1024 {
            return Some(String::from_utf8_lossy(&buffer).into_owned());
        }
    }
}

/// Plain 200 origin for direct-connection cases.
struct PlainOrigin {
    address: std::net::SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl PlainOrigin {
    async fn spawn() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("origin listener should bind");
        let address = listener
            .local_addr()
            .expect("origin listener should have an address");
        let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => return,
                    accepted = listener.accept() => {
                        let Ok((mut stream, _)) = accepted else { continue };
                        tokio::spawn(async move {
                            let Some(head) = read_http_head(&mut stream).await else { return };
                            if !head.starts_with("CONNECT") {
                                use tokio::io::AsyncWriteExt as _;
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
            shutdown,
            task,
        }
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

/// A TLS origin that records the SNI of every ClientHello.
struct TlsSniOrigin {
    address: std::net::SocketAddr,
    snis: Arc<std::sync::Mutex<Vec<Option<String>>>>,
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl TlsSniOrigin {
    async fn spawn() -> Self {
        codex_utils_rustls_provider::ensure_rustls_crypto_provider();
        let certified_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("self-signed certificate");
        let provider = rustls::crypto::CryptoProvider::get_default()
            .expect("rustls crypto provider installed")
            .clone();
        let signing_key = provider
            .key_provider
            .load_private_key(rustls::pki_types::PrivateKeyDer::from(
                certified_key.signing_key,
            ))
            .expect("load signing key");
        let rustls_key = Arc::new(rustls::sign::CertifiedKey::new(
            vec![rustls::pki_types::CertificateDer::from(
                certified_key.cert.der().as_ref().to_vec(),
            )],
            signing_key,
        ));
        let snis = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));
        #[derive(Debug)]
        struct SniffingResolver {
            key: Arc<rustls::sign::CertifiedKey>,
            snis: Arc<std::sync::Mutex<Vec<Option<String>>>>,
        }
        impl rustls::server::ResolvesServerCert for SniffingResolver {
            fn resolve(
                &self,
                client_hello: rustls::server::ClientHello,
            ) -> Option<Arc<rustls::sign::CertifiedKey>> {
                self.snis
                    .lock()
                    .expect("sni lock")
                    .push(client_hello.server_name().map(str::to_string));
                Some(Arc::clone(&self.key))
            }
        }
        let tls_config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(SniffingResolver {
                    key: rustls_key,
                    snis: Arc::clone(&snis),
                })),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TLS origin listener should bind");
        let address = listener
            .local_addr()
            .expect("TLS origin listener should have an address");
        let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(tls_config);
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => return,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { continue };
                        let acceptor = acceptor.clone();
                        tokio::spawn(async move {
                            // The ClientHello (and its SNI) is recorded by the
                            // resolver during the handshake; a client that
                            // then rejects the self-signed certificate still
                            // leaves the observation behind.
                            let _ = acceptor.accept(socket).await;
                        });
                    }
                }
            }
        });
        Self {
            address,
            snis,
            shutdown,
            task,
        }
    }

    fn reset(&self) -> usize {
        self.snis.lock().expect("sni lock").len()
    }

    fn sni_since(&self, marker: usize) -> Vec<Option<String>> {
        self.snis.lock().expect("sni lock")[marker..].to_vec()
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}
