use std::io;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use pretty_assertions::assert_eq;
use rcgen::CertifiedKey;
use rcgen::generate_simple_self_signed;
use rustls::crypto::cipher::OutboundChunks;
use rustls::crypto::cipher::OutboundPlainMessage;
use rustls_pki_types::PrivateKeyDer;

use crate::ClientRouteClass;
use crate::HttpClientBuilder;
use crate::HttpClientFactory;
use crate::HttpClientTlsConfig;
use crate::OutboundProxyPolicy;
use crate::RouteAwareRequestError;
use crate::tls_backend_fallback::should_retry_with_rustls;

#[tokio::test]
async fn recognizes_rustls_protocol_alert_during_connection_establishment() {
    let (url, attempts, stop_server) =
        super::spawn_protocol_version_rejection_server(/*maximum_attempts*/ 1)
            .expect("start TLS alert server");
    codex_utils_rustls_provider::ensure_rustls_crypto_provider();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .no_proxy()
        .build()
        .expect("build rustls client");
    let error = client.post(url).send().await.expect_err("TLS alert");
    let _ = stop_server.send(());
    assert_eq!(
        (error.is_connect(), should_retry_with_rustls(&error)),
        (true, true),
        "a rustls protocol alert in an io payload is a connect failure: {error:?}"
    );
    assert_eq!(attempts.recv().unwrap().unwrap(), 1);
}

#[tokio::test]
async fn does_not_replay_a_post_consumed_before_a_tls_protocol_alert() {
    #[cfg(target_os = "macos")]
    super::warm_secure_transport_once();
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let tls = HttpClientTlsConfig::default()
        .with_root_certificate_pem(cert.pem().as_bytes())
        .unwrap();
    let mut provider = rustls::crypto::aws_lc_rs::default_provider();
    provider.cipher_suites =
        vec![rustls::crypto::aws_lc_rs::cipher_suite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256];
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], PrivateKeyDer::from(signing_key))
        .unwrap();
    config.enable_secret_extraction = true;
    let config = Arc::new(config);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(/*nonblocking*/ true).unwrap();
    let (stop_tx, stop_rx) = mpsc::channel();
    let server = thread::spawn(move || -> io::Result<Vec<Vec<u8>>> {
        let deadline = Instant::now() + Duration::from_secs(/*secs*/ 5);
        let mut bodies = Vec::new();
        while bodies.len() < 2 && Instant::now() < deadline {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            let (socket, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(/*millis*/ 10));
                    continue;
                }
                Err(error) => return Err(error),
            };
            socket.set_nonblocking(/*nonblocking*/ false)?;
            socket.set_read_timeout(Some(Duration::from_secs(/*secs*/ 5)))?;
            socket.set_write_timeout(Some(Duration::from_secs(/*secs*/ 5)))?;
            let connection =
                rustls::ServerConnection::new(Arc::clone(&config)).map_err(io::Error::other)?;
            let mut tls = rustls::StreamOwned::new(connection, socket);
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\nabc") {
                let mut chunk = [0_u8; 1024];
                let n = tls.read(&mut chunk)?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "peer closed before sending the complete POST",
                    ));
                }
                request.extend_from_slice(&chunk[..n]);
            }
            bodies.push(request[request.len() - 3..].to_vec());
            if bodies.len() == 2 {
                // A replay would succeed, making the test fail without
                // relying on a second handshake failure or a timeout.
                tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")?;
                tls.flush()?;
                break;
            }
            let Some(rustls::SupportedCipherSuite::Tls12(suite)) =
                tls.conn.negotiated_cipher_suite()
            else {
                return Err(io::Error::other("expected TLS 1.2 cipher suite"));
            };
            let (sequence, secrets) = tls
                .conn
                .dangerous_extract_secrets()
                .map_err(io::Error::other)?
                .tx;
            let rustls::ConnectionTrafficSecrets::Aes128Gcm { key, iv } = secrets else {
                return Err(io::Error::other("expected AES-128-GCM traffic secrets"));
            };
            // Emit an authenticated alert on the established connection,
            // after the server has consumed the request's entire body.
            let (fixed_iv, explicit_nonce) = iv.as_ref().split_at(/*mid*/ 4);
            let alert = suite
                .aead_alg
                .encrypter(key, fixed_iv, explicit_nonce)
                .encrypt(
                    OutboundPlainMessage {
                        typ: rustls::ContentType::Alert,
                        version: rustls::ProtocolVersion::TLSv1_2,
                        payload: OutboundChunks::Single(&[2, 70]),
                    },
                    sequence,
                )
                .map_err(io::Error::other)?
                .encode();
            tls.sock.write_all(&alert)?;
        }
        Ok(bodies)
    });

    // Explicit roots exercise the pool's normal TLS construction while
    // keeping environment-selected CA bundles out of this native-TLS case.
    let client = HttpClientBuilder::new().build_with_tls(
        &HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        ClientRouteClass::Other,
        tls,
    );
    let result = client
        .post(format!("https://{address}/mcp"))
        .body("abc")
        .timeout(Duration::from_secs(/*secs*/ 5))
        .send()
        .await;
    let _ = stop_tx.send(());
    let bodies = server.join().unwrap().unwrap();
    let RouteAwareRequestError::Request(error) = result.expect_err("post-send TLS failure") else {
        panic!("expected a request error");
    };
    assert_eq!(
        (error.is_connect(), should_retry_with_rustls(&error), bodies),
        (false, false, vec![b"abc".to_vec()]),
        "a post-send protocol alert cannot replay the consumed request: {error:?}"
    );
}
