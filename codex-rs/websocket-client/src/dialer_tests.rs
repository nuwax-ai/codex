use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_http_client::OutboundProxyRoute;
use codex_utils_rustls_provider::ensure_rustls_crypto_provider;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use rcgen::CertifiedKey;
use rcgen::generate_simple_self_signed;
use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::PrivatePkcs8KeyDer;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::proxy::ProxyScheme;

use super::*;
use crate::AsyncIo;
use crate::WebSocketConnection;
use crate::WebSocketConnector;
use crate::WebSocketTlsMode;

#[tokio::test]
async fn public_connector_uses_factory_and_exposes_stream_and_sink() {
    let (target_addr, target_task) = start_echo_websocket_server(/*acceptor*/ None).await;
    let request = format!("ws://localhost:{}/v1/responses", target_addr.port())
        .into_client_request()
        .expect("websocket request should build");
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let connector = WebSocketConnector::new(&factory).expect("connector should build");

    let (mut websocket, _) = connector
        .connect(request, WebSocketConfig::default())
        .await
        .expect("websocket handshake should succeed");
    assert!(!websocket_tcp_nodelay(&websocket));
    let expected = Message::Text("hello".into());
    websocket
        .send(expected.clone())
        .await
        .expect("websocket should send");
    let actual = websocket
        .next()
        .await
        .expect("websocket should receive a message")
        .expect("websocket message should be valid");
    assert_eq!(actual, expected);

    target_task.await.expect("target task should finish");
}

#[tokio::test]
async fn public_connector_enables_tcp_nodelay_when_requested() {
    let (target_addr, target_task) = start_echo_websocket_server(/*acceptor*/ None).await;
    let request = format!("ws://localhost:{}/v1/responses", target_addr.port())
        .into_client_request()
        .expect("websocket request should build");
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let connector = WebSocketConnector::new(&factory)
        .expect("connector should build")
        .with_tcp_nodelay();

    let (mut websocket, _) = connector
        .connect(request, WebSocketConfig::default())
        .await
        .expect("websocket handshake should succeed");
    assert!(websocket_tcp_nodelay(&websocket));
    let expected = Message::Text("latency-sensitive".into());
    websocket
        .send(expected.clone())
        .await
        .expect("websocket should send");
    assert_eq!(
        websocket
            .next()
            .await
            .expect("websocket should receive a message")
            .expect("websocket message should be valid"),
        expected
    );

    target_task.await.expect("target task should finish");
}

#[tokio::test]
async fn tungstenite_default_tls_mode_ignores_invalid_custom_ca_in_a_subprocess() {
    let (target_addr, target_task) = start_echo_websocket_server(/*acceptor*/ None).await;
    let target_url = format!("ws://127.0.0.1:{}/v1/responses", target_addr.port());
    let executable = std::env::current_exe().expect("test executable should be available");
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(executable);
        command.args([
            "--exact",
            "dialer::tests::tungstenite_default_tls_mode_subprocess_probe",
            "--nocapture",
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
            "SSL_CERT_FILE",
        ] {
            command.env_remove(key);
        }
        command
            .env(
                "CODEX_CA_CERTIFICATE",
                "/codex-websocket-client-nonexistent-custom-ca.pem",
            )
            .env("CODEX_WEBSOCKET_DEFAULT_TLS_PROBE_URL", target_url)
            .output()
            .expect("WebSocket default-TLS subprocess should run")
    })
    .await
    .expect("WebSocket default-TLS subprocess should join");

    assert!(
        output.status.success(),
        "WebSocket default-TLS subprocess failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    target_task.await.expect("target task should finish");
}

#[tokio::test]
async fn tungstenite_default_tls_mode_subprocess_probe() {
    let Ok(url) = std::env::var("CODEX_WEBSOCKET_DEFAULT_TLS_PROBE_URL") else {
        return;
    };
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    assert!(
        WebSocketConnector::new(&factory).is_err(),
        "explicit Codex TLS should reject the invalid custom CA"
    );
    let connector =
        WebSocketConnector::new_with_tls_mode(&factory, WebSocketTlsMode::TungsteniteDefault)
            .expect("Tungstenite-default TLS should ignore custom CA configuration");
    let request = url
        .into_client_request()
        .expect("websocket request should build");
    let (mut websocket, _) = connector
        .connect(request, WebSocketConfig::default())
        .await
        .expect("WebSocket should connect without constructing Codex TLS");
    let expected = Message::Text("Tungstenite default TLS".into());
    websocket
        .send(expected.clone())
        .await
        .expect("WebSocket should send");
    assert_eq!(
        websocket
            .next()
            .await
            .expect("WebSocket should receive a message")
            .expect("WebSocket message should be valid"),
        expected
    );
}

#[tokio::test]
async fn direct_route_connects_secure_websocket() {
    let (tls_config, acceptor, _) = test_tls_configs();
    let (target_addr, target_task) = start_tls_websocket_server(acceptor).await;
    let request = format!("wss://localhost:{}/v1/responses", target_addr.port())
        .into_client_request()
        .expect("websocket request should build");

    let (inner, _) = connect(
        request,
        WebSocketConfig::default(),
        Some(tls_config),
        OutboundProxyRoute::Direct,
        TcpNodelay::Enabled,
        /*loopback_direct*/ false,
    )
    .await
    .expect("direct websocket handshake should succeed");
    drop(test_connection(inner));

    target_task.await.expect("target task should finish");
}

#[tokio::test]
async fn http_proxy_tunnels_secure_websocket_before_handshake() {
    assert_proxy_tunnels_secure_websocket(/*proxy_tls*/ false).await;
}

#[tokio::test]
async fn https_proxy_tunnels_secure_websocket_before_handshake() {
    assert_proxy_tunnels_secure_websocket(/*proxy_tls*/ true).await;
}

#[tokio::test]
async fn environment_proxy_route_honors_no_proxy_in_a_subprocess() {
    assert_no_proxy_subprocess(
        "127.0.0.1",
        /*expect_proxy*/ false,
        /*proxy_tls*/ false,
    )
    .await;
    assert_no_proxy_subprocess(
        "unrelated.example",
        /*expect_proxy*/ true,
        /*proxy_tls*/ false,
    )
    .await;
    assert_no_proxy_subprocess(
        "unrelated.example",
        /*expect_proxy*/ true,
        /*proxy_tls*/ true,
    )
    .await;
}

#[tokio::test]
async fn transport_default_routes_literal_ip_websockets_through_environment_proxies() {
    let (target_addr, target_task) = start_echo_websocket_server(/*acceptor*/ None).await;
    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("proxy listener should bind");
    let proxy_addr = proxy_listener
        .local_addr()
        .expect("proxy listener should have an address");
    let proxy_task = tokio::spawn(async move {
        let (mut client, _) = proxy_listener.accept().await.expect("proxy should accept");
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            client
                .read_exact(&mut byte)
                .await
                .expect("proxy should read CONNECT request");
            request.push(byte[0]);
        }
        let mut target = tokio::net::TcpStream::connect(target_addr)
            .await
            .expect("proxy should connect to target");
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .expect("proxy should acknowledge CONNECT");
        let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
        String::from_utf8(request).expect("CONNECT request should be UTF-8")
    });
    let target_url = format!("ws://127.0.0.1:{}/v1/responses", target_addr.port());
    let proxy_url = format!("http://localhost:{}", proxy_addr.port());
    let executable = std::env::current_exe().expect("test executable should be available");
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(executable);
        command.args([
            "--exact",
            "dialer::tests::environment_proxy_literal_ip_subprocess_probe",
            "--nocapture",
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
        ] {
            command.env_remove(key);
        }
        command
            .env("HTTP_PROXY", &proxy_url)
            .env("CODEX_WEBSOCKET_ENV_PROXY_PROBE_URL", target_url)
            .output()
            .expect("WebSocket environment-proxy subprocess should run")
    })
    .await
    .expect("WebSocket environment-proxy subprocess should join");
    assert!(
        output.status.success(),
        "WebSocket environment-proxy subprocess failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    target_task.await.expect("target task should finish");
    let request = proxy_task.await.expect("proxy task should finish");
    let expected_request_line = format!("CONNECT 127.0.0.1:{} HTTP/1.1", target_addr.port());
    assert_eq!(
        request.lines().next(),
        Some(expected_request_line.as_str()),
        "literal-IP WebSocket destinations must reach the configured environment proxy"
    );
}

#[tokio::test]
async fn no_proxy_subprocess_probe() {
    let Ok(url) = std::env::var("CODEX_WEBSOCKET_NO_PROXY_PROBE_URL") else {
        return;
    };
    let proxy_url = std::env::var("CODEX_WEBSOCKET_NO_PROXY_PROBE_PROXY")
        .expect("parent test should provide a proxy URL");
    let no_proxy = std::env::var("NO_PROXY").expect("parent test should provide a no-proxy value");
    let request = url
        .into_client_request()
        .expect("websocket request should build");
    let tls_config =
        if let Ok(certificate_hex) = std::env::var("CODEX_WEBSOCKET_NO_PROXY_PROBE_CA_DER") {
            ensure_rustls_crypto_provider();
            assert_eq!(
                certificate_hex.len() % 2,
                0,
                "encoded certificate should contain complete bytes"
            );
            let certificate = (0..certificate_hex.len())
                .step_by(2)
                .map(|index| {
                    u8::from_str_radix(&certificate_hex[index..index + 2], 16)
                        .expect("encoded certificate should contain hexadecimal bytes")
                })
                .collect::<Vec<_>>();
            let mut roots = RootCertStore::empty();
            roots
                .add(CertificateDer::from(certificate))
                .expect("proxy certificate should be trusted");
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        } else {
            test_tls_configs().0
        };
    let (inner, _) = connect(
        request,
        WebSocketConfig::default(),
        Some(tls_config),
        OutboundProxyRoute::Proxy {
            url: proxy_url,
            no_proxy: Some(no_proxy),
        },
        TcpNodelay::Enabled,
        /*loopback_direct*/ false,
    )
    .await
    .expect("websocket handshake should succeed");
    let mut websocket = test_connection(inner);
    websocket
        .send(Message::Text("probe".into()))
        .await
        .expect("probe should send");
    assert_eq!(
        websocket
            .next()
            .await
            .expect("probe should receive a message")
            .expect("probe message should be valid"),
        Message::Text("probe".into())
    );
}

#[tokio::test]
async fn environment_proxy_literal_ip_subprocess_probe() {
    let Ok(url) = std::env::var("CODEX_WEBSOCKET_ENV_PROXY_PROBE_URL") else {
        return;
    };
    // Drive the connector-level path: the factory resolves the route for the
    // literal-IP destination and the transport must honor HTTP_PROXY.
    // Tungstenite's default TLS connector has no implicit crypto provider, so
    // install the process default before wss dials reach it.
    ensure_rustls_crypto_provider();
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let connector =
        WebSocketConnector::new_with_tls_mode(&factory, WebSocketTlsMode::TungsteniteDefault)
            .expect("connector should build without a custom CA");
    let request = url
        .into_client_request()
        .expect("websocket request should build");
    let (mut websocket, _) = connector
        .connect(request, WebSocketConfig::default())
        .await
        .expect("websocket handshake should succeed through the environment proxy");
    websocket
        .send(Message::Text("probe".into()))
        .await
        .expect("probe should send");
    assert_eq!(
        websocket
            .next()
            .await
            .expect("probe should receive a message")
            .expect("probe message should be valid"),
        Message::Text("probe".into())
    );
}

#[test]
fn https_proxy_defaults_to_port_443_and_preserves_explicit_port() {
    let default_port = ProxyEndpoint::parse("https://proxy.example")
        .expect("HTTPS proxy without a port should parse");
    let explicit_port = ProxyEndpoint::parse("https://proxy.example:8443")
        .expect("HTTPS proxy with a port should parse");

    assert_eq!(
        default_port,
        ProxyEndpoint {
            config: ProxyConfig {
                scheme: ProxyScheme::Http,
                host: "proxy.example".to_string(),
                port: 443,
                auth: None,
            },
            tls: true,
        }
    );
    assert_eq!(
        explicit_port,
        ProxyEndpoint {
            config: ProxyConfig {
                scheme: ProxyScheme::Http,
                host: "proxy.example".to_string(),
                port: 8443,
                auth: None,
            },
            tls: true,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn happy_eyeballs_does_not_wait_for_stalled_preferred_family() {
    let stalled = "[2001:db8::1]:443"
        .parse::<SocketAddr>()
        .expect("stalled address should parse");
    let reachable = "127.0.0.1:443"
        .parse::<SocketAddr>()
        .expect("reachable address should parse");

    let connected = tokio::time::timeout(
        Duration::from_secs(1),
        connect_happy_eyeballs(vec![stalled, reachable], |address| async move {
            if address == stalled {
                std::future::pending::<()>().await;
            }
            Ok(address)
        }),
    )
    .await
    .expect("alternate family should start before timeout")
    .expect("alternate family should connect");

    assert_eq!(connected, reachable);
}

#[test]
fn loopback_direct_drops_non_loopback_resolved_addresses() {
    let loopback = "127.0.0.1:8080"
        .parse::<SocketAddr>()
        .expect("loopback address should parse");
    let remote = "192.0.2.1:8080"
        .parse::<SocketAddr>()
        .expect("remote address should parse");

    assert_eq!(
        loopback_addresses(vec![remote, loopback]).expect("loopback result should remain"),
        vec![loopback]
    );
}

#[test]
fn loopback_direct_rejects_localhost_resolution_without_loopback_addresses() {
    let remote = "192.0.2.1:8080"
        .parse::<SocketAddr>()
        .expect("remote address should parse");

    let error = loopback_addresses(vec![remote])
        .expect_err("localhost resolution without loopback addresses must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[tokio::test]
async fn routed_tcp_connections_only_enable_nodelay_when_requested() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener
        .local_addr()
        .expect("listener should have an address")
        .to_string();

    let default_stream = connect_tcp(address.clone(), TcpNodelay::Default)
        .await
        .expect("default TCP connection should succeed");
    assert!(!default_stream.nodelay().expect("TCP_NODELAY should read"));

    let low_latency_stream = connect_tcp(address, TcpNodelay::Enabled)
        .await
        .expect("low-latency TCP connection should succeed");
    assert!(
        low_latency_stream
            .nodelay()
            .expect("TCP_NODELAY should read")
    );
}

fn websocket_tcp_nodelay(websocket: &WebSocketConnection) -> bool {
    let Some(ConnectionInner::Left(stream)) = &websocket.inner else {
        panic!("default connector should use Tungstenite's transport");
    };
    let MaybeTlsStream::Plain(stream) = stream.get_ref() else {
        panic!("test websocket should use a plain TCP stream");
    };
    stream.nodelay().expect("TCP_NODELAY should read")
}

async fn start_echo_websocket_server(
    acceptor: Option<TlsAcceptor>,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("target listener should bind");
    let address = listener
        .local_addr()
        .expect("target listener should have an address");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("target should accept");
        let stream: Box<dyn AsyncIo> = match acceptor {
            Some(acceptor) => Box::new(
                acceptor
                    .accept(stream)
                    .await
                    .expect("target TLS handshake should succeed"),
            ),
            None => Box::new(stream),
        };
        let mut websocket = accept_async(stream)
            .await
            .expect("target websocket handshake should succeed");
        let message = websocket
            .next()
            .await
            .expect("target should receive a message")
            .expect("target websocket message should be valid");
        websocket
            .send(message)
            .await
            .expect("target should echo the message");
    });
    (address, task)
}

async fn assert_no_proxy_subprocess(no_proxy: &str, expect_proxy: bool, proxy_tls: bool) {
    let (target_acceptor, proxy_acceptor, certificate) = if proxy_tls {
        let (_, acceptor, certificate) = test_tls_configs();
        (Some(acceptor.clone()), Some(acceptor), Some(certificate))
    } else {
        (None, None, None)
    };
    let (target_addr, target_task) = start_echo_websocket_server(target_acceptor).await;
    let proxy_listener = Arc::new(
        TcpListener::bind("127.0.0.1:0")
            .await
            .expect("proxy listener should bind"),
    );
    let proxy_addr = proxy_listener
        .local_addr()
        .expect("proxy listener should have an address");
    let proxy_task = if expect_proxy {
        let proxy_listener = Arc::clone(&proxy_listener);
        Some(tokio::spawn(async move {
            let (client, _) = proxy_listener.accept().await.expect("proxy should accept");
            let mut client: Box<dyn AsyncIo> = match proxy_acceptor {
                Some(acceptor) => Box::new(
                    acceptor
                        .accept(client)
                        .await
                        .expect("proxy TLS handshake should succeed"),
                ),
                None => Box::new(client),
            };
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                client
                    .read_exact(&mut byte)
                    .await
                    .expect("proxy should read CONNECT request");
                request.push(byte[0]);
            }
            let mut target = tokio::net::TcpStream::connect(target_addr)
                .await
                .expect("proxy should connect to target");
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .expect("proxy should acknowledge CONNECT");
            let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
            String::from_utf8(request).expect("CONNECT request should be UTF-8")
        }))
    } else {
        None
    };
    let executable = std::env::current_exe().expect("test executable should be available");
    let target_scheme = if proxy_tls { "wss" } else { "ws" };
    let proxy_scheme = if proxy_tls { "https" } else { "http" };
    let target_host = if proxy_tls { "localhost" } else { "127.0.0.1" };
    let target_url = format!(
        "{target_scheme}://{target_host}:{}/v1/responses",
        target_addr.port()
    );
    let proxy_url = format!("{proxy_scheme}://localhost:{}", proxy_addr.port());
    let no_proxy = no_proxy.to_string();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(executable);
        command.args([
            "--exact",
            "dialer::tests::no_proxy_subprocess_probe",
            "--nocapture",
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
        ] {
            command.env_remove(key);
        }
        command
            .env(
                if proxy_tls {
                    "HTTPS_PROXY"
                } else {
                    "HTTP_PROXY"
                },
                &proxy_url,
            )
            .env("NO_PROXY", no_proxy)
            .env("CODEX_WEBSOCKET_NO_PROXY_PROBE_URL", target_url)
            .env("CODEX_WEBSOCKET_NO_PROXY_PROBE_PROXY", proxy_url);
        command.env_remove("CODEX_WEBSOCKET_NO_PROXY_PROBE_CA_DER");
        if let Some(certificate) = certificate {
            let certificate_hex = certificate
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            command.env("CODEX_WEBSOCKET_NO_PROXY_PROBE_CA_DER", certificate_hex);
        }
        command
            .output()
            .expect("WebSocket no-proxy subprocess should run")
    })
    .await
    .expect("WebSocket no-proxy subprocess should join");
    assert!(
        output.status.success(),
        "WebSocket no-proxy subprocess failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    target_task.await.expect("target task should finish");
    // In the bypass case no task services the proxy listener, so the successful child connection
    // above also proves that the matching NO_PROXY value selected the target directly.
    if let Some(proxy_task) = proxy_task {
        let request = proxy_task.await.expect("proxy task should finish");
        let expected_request_line =
            format!("CONNECT {target_host}:{} HTTP/1.1", target_addr.port());
        assert_eq!(request.lines().next(), Some(expected_request_line.as_str()));
    }
}

async fn assert_proxy_tunnels_secure_websocket(proxy_tls: bool) {
    let (tls_config, acceptor, _) = test_tls_configs();
    let (target_addr, target_task) = start_tls_websocket_server(acceptor.clone()).await;

    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("proxy listener should bind");
    let proxy_addr = proxy_listener
        .local_addr()
        .expect("proxy listener should have an address");
    let connect_request = Arc::new(Mutex::new(None));
    let proxy_connect_request = Arc::clone(&connect_request);
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await.expect("proxy should accept");
        let mut client: Box<dyn AsyncIo> = if proxy_tls {
            Box::new(
                acceptor
                    .accept(client)
                    .await
                    .expect("proxy TLS handshake should succeed"),
            )
        } else {
            Box::new(client)
        };
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            client
                .read_exact(&mut byte)
                .await
                .expect("proxy should read CONNECT request");
            request.push(byte[0]);
        }
        *proxy_connect_request.lock().await =
            Some(String::from_utf8(request).expect("CONNECT request should contain valid UTF-8"));

        let mut target = tokio::net::TcpStream::connect(target_addr)
            .await
            .expect("proxy should connect to target");
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .expect("proxy should acknowledge CONNECT");
        let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
    });

    let target_authority = format!("localhost:{}", target_addr.port());
    let proxy_scheme = if proxy_tls { "https" } else { "http" };
    let request = format!("wss://{target_authority}/v1/responses")
        .into_client_request()
        .expect("websocket request should build");
    let (inner, _) = connect(
        request,
        WebSocketConfig::default(),
        Some(tls_config),
        OutboundProxyRoute::Proxy {
            url: format!("{proxy_scheme}://localhost:{}", proxy_addr.port()),
            no_proxy: None,
        },
        TcpNodelay::Enabled,
        /*loopback_direct*/ false,
    )
    .await
    .expect("proxied websocket handshake should succeed");
    drop(test_connection(inner));

    target_task.await.expect("target task should finish");
    proxy_task.await.expect("proxy task should finish");
    let request = connect_request
        .lock()
        .await
        .clone()
        .expect("proxy should record CONNECT request");
    let expected_request_line = format!("CONNECT {target_authority} HTTP/1.1");
    assert_eq!(request.lines().next(), Some(expected_request_line.as_str()));
}

async fn start_tls_websocket_server(acceptor: TlsAcceptor) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("target listener should bind");
    let address = listener
        .local_addr()
        .expect("target listener should have an address");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("target should accept");
        let stream = acceptor
            .accept(stream)
            .await
            .expect("target TLS handshake should succeed");
        let mut websocket = accept_async(stream)
            .await
            .expect("target websocket handshake should succeed");
        let _ = websocket.close(None).await;
    });
    (address, task)
}

fn test_tls_configs() -> (Arc<ClientConfig>, TlsAcceptor, CertificateDer<'static>) {
    ensure_rustls_crypto_provider();
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("test certificate should generate");
    let certificate = cert.der().clone();
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], private_key)
        .expect("test server config should build");

    let mut roots = RootCertStore::empty();
    roots
        .add(certificate.clone())
        .expect("test certificate should be trusted");
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    (
        Arc::new(client_config),
        TlsAcceptor::from(Arc::new(server_config)),
        certificate,
    )
}

fn test_connection(inner: ConnectionInner) -> WebSocketConnection {
    let policy = codex_http_client::NetworkPolicy::unmanaged();
    let lease = policy
        .acquire(&url::Url::parse("wss://localhost/").unwrap())
        .unwrap();
    WebSocketConnection::new(inner, lease)
}

/// One CA trusted for both the TLS proxy and the TLS target, plus its PEM for
/// child processes (`SSL_CERT_FILE` feeds both the dialer's proxy-TLS build
/// and tungstenite's native-roots default connector).
fn test_tls_acceptor_and_pem() -> (TlsAcceptor, String) {
    ensure_rustls_crypto_provider();
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("test certificate should generate");
    let certificate = cert.der().clone();
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)
        .expect("test server config should build");
    (TlsAcceptor::from(Arc::new(server_config)), cert.pem())
}

/// Drives the connector-level default (ReqwestDefault) path in a subprocess
/// and asserts the environment proxy actually tunnels the websocket: the
/// proxy observes one CONNECT for the real target before the echo round-trip.
async fn assert_env_proxy_tunnel_subprocess(
    target_tls: bool,
    proxy_tls: bool,
    env_var: &str,
    target_host: &str,
) {
    // One CA backs every TLS party in the case (target, proxy, or both) so a
    // single SSL_CERT_FILE in the child trusts the whole chain.
    let (shared_acceptor, shared_ca_pem) = test_tls_acceptor_and_pem();
    let target_acceptor = target_tls.then(|| shared_acceptor.clone());
    let proxy_acceptor = proxy_tls.then_some(shared_acceptor);
    let ca_pem = (target_tls || proxy_tls).then_some(shared_ca_pem);
    let (target_addr, target_task) = start_echo_websocket_server(target_acceptor).await;
    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("proxy listener should bind");
    let proxy_addr = proxy_listener
        .local_addr()
        .expect("proxy listener should have an address");
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await.expect("proxy should accept");
        let mut client: Box<dyn AsyncIo> = match proxy_acceptor {
            Some(acceptor) => Box::new(
                acceptor
                    .accept(client)
                    .await
                    .expect("proxy TLS handshake should succeed"),
            ),
            None => Box::new(client),
        };
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            client
                .read_exact(&mut byte)
                .await
                .expect("proxy should read CONNECT request");
            request.push(byte[0]);
        }
        let mut target = tokio::net::TcpStream::connect(target_addr)
            .await
            .expect("proxy should connect to target");
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .expect("proxy should acknowledge CONNECT");
        let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
        String::from_utf8(request).expect("CONNECT request should be UTF-8")
    });
    let target_scheme = if target_tls { "wss" } else { "ws" };
    let proxy_scheme = if proxy_tls { "https" } else { "http" };
    let target_url = format!("{target_scheme}://{target_host}:{}/v1/responses", target_addr.port());
    let proxy_url = format!("{proxy_scheme}://localhost:{}", proxy_addr.port());
    let env_var = env_var.to_string();
    let executable = std::env::current_exe().expect("test executable should be available");
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(executable);
        command.args([
            "--exact",
            "dialer::tests::environment_proxy_literal_ip_subprocess_probe",
            "--nocapture",
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
            "CODEX_CA_CERTIFICATE",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            command.env_remove(key);
        }
        command.env(&env_var, &proxy_url);
        command.env("CODEX_WEBSOCKET_ENV_PROXY_PROBE_URL", &target_url);
        if let Some(ca_pem) = ca_pem.as_ref() {
            let ca_path = std::env::temp_dir().join(format!(
                "codex-ws-env-proxy-ca-{}.pem",
                proxy_addr.port()
            ));
            std::fs::write(&ca_path, ca_pem).expect("CA bundle should be written");
            command.env("SSL_CERT_FILE", &ca_path);
        }
        command
            .output()
            .expect("WebSocket environment-proxy subprocess should run")
    })
    .await
    .expect("WebSocket environment-proxy subprocess should join");
    assert!(
        output.status.success(),
        "WebSocket environment-proxy subprocess failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    target_task.await.expect("target task should finish");
    let request = proxy_task.await.expect("proxy task should finish");
    let expected_request_line = format!("CONNECT {target_host}:{} HTTP/1.1", target_addr.port());
    assert_eq!(
        request.lines().next(),
        Some(expected_request_line.as_str()),
        "the environment proxy must tunnel exactly one CONNECT for the real target"
    );
}

#[tokio::test]
async fn default_environment_https_proxy_dials_through_tls_to_proxy() {
    // wss destination + HTTPS_PROXY=https://... — the SDK's own resolver
    // rejects this scheme, so the dial must fall through to TLS-to-proxy.
    assert_env_proxy_tunnel_subprocess(
        /*target_tls*/ true,
        /*proxy_tls*/ true,
        "HTTPS_PROXY",
        "localhost",
    )
    .await;
    // A plain ws destination still tunnels when ALL_PROXY selects https.
    assert_env_proxy_tunnel_subprocess(
        /*target_tls*/ false,
        /*proxy_tls*/ true,
        "ALL_PROXY",
        "127.0.0.1",
    )
    .await;
    // wss through a plain http environment proxy stays on the SDK's native
    // CONNECT path (no TLS to the proxy itself).
    assert_env_proxy_tunnel_subprocess(
        /*target_tls*/ true,
        /*proxy_tls*/ false,
        "HTTP_PROXY",
        "localhost",
    )
    .await;
}

/// Minimal SOCKS5 mock: method negotiation (no auth), one CONNECT request,
/// then a bidirectional tunnel. Asserts the relayed address type and bytes.
async fn assert_socks5_env_proxy_tunnel_subprocess() {
    let (target_addr, target_task) = start_echo_websocket_server(/*acceptor*/ None).await;
    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("socks5 listener should bind");
    let proxy_addr = proxy_listener
        .local_addr()
        .expect("socks5 listener should have an address");
    let proxy_task = tokio::spawn(async move {
        let (mut client, _) = proxy_listener.accept().await.expect("socks5 should accept");
        let mut greeting = [0_u8; 2];
        client.read_exact(&mut greeting).await.expect("greeting");
        assert_eq!(greeting[0], 0x05, "SOCKS5 version");
        let mut methods = vec![0_u8; greeting[1] as usize];
        client
            .read_exact(&mut methods)
            .await
            .expect("supported methods");
        client
            .write_all(&[0x05, 0x00])
            .await
            .expect("no-auth method selection");
        let mut header = [0_u8; 4];
        client
            .read_exact(&mut header)
            .await
            .expect("connect request header");
        assert_eq!(&header[..3], &[0x05, 0x01, 0x00], "connect command");
        let target_octets = match header[3] {
            0x01 => {
                let mut octets = [0_u8; 4];
                client
                    .read_exact(&mut octets)
                    .await
                    .expect("IPv4 address");
                octets.to_vec()
            }
            address_type => panic!("unexpected SOCKS5 address type {address_type}"),
        };
        let mut port = [0_u8; 2];
        client.read_exact(&mut port).await.expect("target port");
        client
            .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .expect("connect success reply");
        let mut target = tokio::net::TcpStream::connect(target_addr)
            .await
            .expect("socks5 should connect to target");
        let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
        (target_octets, u16::from_be_bytes(port))
    });
    let target_url = format!("ws://127.0.0.1:{}/v1/responses", target_addr.port());
    let proxy_url = format!("socks5://localhost:{}", proxy_addr.port());
    let executable = std::env::current_exe().expect("test executable should be available");
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(executable);
        command.args([
            "--exact",
            "dialer::tests::environment_proxy_literal_ip_subprocess_probe",
            "--nocapture",
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
        ] {
            command.env_remove(key);
        }
        command
            .env("ALL_PROXY", &proxy_url)
            .env("CODEX_WEBSOCKET_ENV_PROXY_PROBE_URL", &target_url)
            .output()
            .expect("WebSocket socks5 subprocess should run")
    })
    .await
    .expect("WebSocket socks5 subprocess should join");
    assert!(
        output.status.success(),
        "WebSocket socks5 subprocess failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    target_task.await.expect("target task should finish");
    let (octets, port) = proxy_task.await.expect("socks5 task should finish");
    assert_eq!(
        (octets.as_slice(), port),
        (
            [127, 0, 0, 1].as_slice(),
            target_addr.port()
        ),
        "the SOCKS5 proxy must relay the real literal-IP target"
    );
}

#[tokio::test]
async fn socks5_environment_proxy_tunnels_plain_websocket() {
    assert_socks5_env_proxy_tunnel_subprocess().await;
}

#[tokio::test]
async fn environment_proxy_redaction_probe() {
    let Ok(url) = std::env::var("CODEX_WEBSOCKET_REDACTION_PROBE_URL") else {
        return;
    };
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let connector =
        WebSocketConnector::new_with_tls_mode(&factory, WebSocketTlsMode::TungsteniteDefault)
            .expect("connector should build without a custom CA");
    let request = url
        .into_client_request()
        .expect("websocket request should build");
    let error = connector
        .connect(request, WebSocketConfig::default())
        .await
        .expect_err("the malformed proxy credentials must fail the dial");
    eprintln!("redaction probe display: {error}");
    eprintln!("redaction probe debug: {error:?}");
    assert!(
        matches!(
            &error,
            tokio_tungstenite::tungstenite::Error::Url(
                tokio_tungstenite::tungstenite::error::UrlError::InvalidProxyConfig(_)
            )
        ),
        "the failure category must stay InvalidProxyConfig, got {error:?}"
    );
}

/// Subprocess boundary: whatever the locked parser embedded in
/// InvalidProxyConfig (truncated percent-escapes, non-UTF-8 decodes) must not
/// surface in Display, Debug, or test output, while the dial still fails.
#[tokio::test]
async fn invalid_proxy_credentials_never_reach_error_surfaces() {
    for password in ["trunc%", "%ff%fe"] {
        let executable = std::env::current_exe().expect("test executable should be available");
        let proxy_url = format!("http://user:{password}@127.0.0.1:1");
        let url = "ws://127.0.0.1:1/v1/responses";
        let password = password.to_string();
        let output = tokio::task::spawn_blocking(move || {
            let mut command = Command::new(executable);
            command.args([
                "--exact",
                "dialer::tests::environment_proxy_redaction_probe",
                "--nocapture",
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
            ] {
                command.env_remove(key);
            }
            command
                .env("HTTP_PROXY", &proxy_url)
                .env("CODEX_WEBSOCKET_REDACTION_PROBE_URL", url)
                .output()
                .expect("WebSocket redaction subprocess should run")
        })
        .await
        .expect("WebSocket redaction subprocess should join");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "redaction subprocess failed for {password}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        for output in [&stdout, &stderr] {
            assert!(
                !output.contains(password.as_str()),
                "raw proxy credentials leaked into subprocess output:\n{output}"
            );
            assert!(
                !output.contains("user:"),
                "proxy userinfo leaked into subprocess output:\n{output}"
            );
        }
        assert!(
            stderr.contains("<redacted>"),
            "the redacted placeholder should describe the failure:\n{stderr}"
        );
    }

    // The explicit-URL parse path already redacts its own payload.
    let error = ProxyEndpoint::parse("http://user:trunc%@proxy.example:1")
        .expect_err("truncated proxy credentials should fail to parse");
    let rendered = format!("{error}");
    assert!(
        !rendered.contains("trunc%"),
        "explicit parse leaked credentials: {rendered}"
    );
    assert!(rendered.contains("<redacted>"));

    // The shared boundary keeps the category and passes other errors through.
    let leak = WebSocketError::Url(UrlError::InvalidProxyConfig("user:leak%".to_string()));
    assert!(matches!(
        &redact_invalid_proxy_config(leak),
        WebSocketError::Url(UrlError::InvalidProxyConfig(payload)) if payload == "<redacted>"
    ));
    let io_failure = WebSocketError::Io(std::io::Error::other("unrelated"));
    assert!(matches!(
        redact_invalid_proxy_config(io_failure),
        WebSocketError::Io(_)
    ));
}
