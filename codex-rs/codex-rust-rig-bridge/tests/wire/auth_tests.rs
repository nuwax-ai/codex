use super::support;
use codex_api::AuthError;
use codex_api::AuthHeadersFuture;
use codex_api::AuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

struct RefreshingAuth {
    calls: AtomicUsize,
}

impl AuthProvider for RefreshingAuth {
    fn add_auth_headers(&self, _: &mut HeaderMap) {
        panic!("outbound requests must use resolved authentication");
    }

    fn resolve_auth_headers(&self) -> AuthHeadersFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::AUTHORIZATION,
                HeaderValue::from_static("bearer refreshed-dummy"),
            );
            headers.insert(
                "x-gateway-auth",
                HeaderValue::from_static("bearer refreshed-dummy"),
            );
            Ok(headers)
        })
    }
}

#[tokio::test]
async fn each_protocol_resolves_auth_once_and_preserves_gateway_headers() {
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        let auth = Arc::new(RefreshingAuth {
            calls: AtomicUsize::new(0),
        });
        let (wire, _, _) = support::capture_with_auth(
            &support::request(vec![support::user()]),
            protocol,
            auth.clone(),
        )
        .await;
        let headers = wire["headers"].as_str().unwrap();
        let primary = match protocol {
            RigProtocol::Chat => "authorization: Bearer refreshed-dummy\r\n",
            RigProtocol::Anthropic => "x-api-key: refreshed-dummy\r\n",
        };
        assert!(headers.contains(primary), "resolved primary header missing");
        assert!(headers.contains("x-gateway-auth: bearer refreshed-dummy\r\n"));
        assert_eq!(auth.calls.load(Ordering::SeqCst), 1);
    }
}

struct ConflictingAuth;

struct GatewayAuth {
    model_key_header: &'static str,
}

impl AuthProvider for GatewayAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic dummy-gateway"),
        );
        headers.insert(
            self.model_key_header,
            HeaderValue::from_static("dummy-model-key"),
        );
    }
}

#[tokio::test]
async fn gateway_authorization_is_preserved_with_separate_model_keys() {
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        for model_key_header in ["api-key", "x-api-key"] {
            let (wire, _, _) = support::capture_with_auth(
                &support::request(vec![support::user()]),
                protocol,
                Arc::new(GatewayAuth { model_key_header }),
            )
            .await;
            let headers = wire["headers"].as_str().unwrap();
            assert!(headers.contains("\r\nauthorization: Basic dummy-gateway\r\n"));
            assert!(!headers.contains("Bearer Basic"));
            assert!(headers.contains(&format!("\r\n{model_key_header}: dummy-model-key\r\n")));
            if protocol == RigProtocol::Anthropic {
                assert!(headers.contains("\r\nx-api-key: dummy-model-key\r\n"));
            }
        }
    }
}

impl AuthProvider for ConflictingAuth {
    fn add_auth_headers(&self, _: &mut HeaderMap) {
        panic!("auth errors must not fall back to the synchronous snapshot");
    }

    fn resolve_auth_headers(&self) -> AuthHeadersFuture<'_> {
        Box::pin(async { Err(AuthError::Build("gateway auth conflict".into())) })
    }
}

#[tokio::test]
async fn auth_resolution_failure_prevents_network_requests() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = super::error_tests::provider(listener.local_addr().unwrap());
    let auth: codex_api::SharedAuthProvider = Arc::new(ConflictingAuth);
    let result = codex_rust_rig_bridge::stream_via_rig(
        &support::request(vec![support::user()]),
        &provider,
        &auth,
        HeaderMap::new(),
        RigProtocol::Chat,
        std::time::Duration::from_secs(1),
    )
    .await;
    assert!(
        matches!(result, Err(codex_api::ApiError::Transport(codex_api::TransportError::Build(message))) if message == "gateway auth conflict")
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}
