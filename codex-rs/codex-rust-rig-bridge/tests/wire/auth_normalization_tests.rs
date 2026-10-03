//! Actual HTTP auth distinguishes a missing credential from an explicit empty one.
use super::support;
use codex_api::ApiError;
use codex_api::AuthProvider;
use codex_api::SharedAuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

struct HeaderAuth(HeaderMap);
impl AuthProvider for HeaderAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        headers.extend(self.0.clone());
    }
}

fn header_map(values: &[(&'static str, &'static str)]) -> HeaderMap {
    values
        .iter()
        .map(|(name, value)| {
            (
                http::HeaderName::from_static(name),
                HeaderValue::from_static(value),
            )
        })
        .collect()
}

fn primary_headers(wire: &serde_json::Value) -> BTreeMap<String, String> {
    wire["headers"]
        .as_str()
        .expect("wire headers")
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "authorization" | "api-key" | "x-api-key"
            )
        })
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect()
}

#[tokio::test]
async fn anonymous_and_gateway_requests_preserve_their_actual_primary_auth() {
    for protocol in [
        RigProtocol::Chat,
        RigProtocol::Anthropic,
        RigProtocol::Responses,
    ] {
        for supplied in [
            Vec::new(),
            vec![("authorization", "Basic gateway")],
            vec![("authorization", "Token gateway")],
            vec![("authorization", "Legacy gateway")],
        ] {
            let (wire, _, _) = support::capture_with_auth(
                &support::request(vec![support::user()]),
                protocol,
                Arc::new(HeaderAuth(header_map(&supplied))),
            )
            .await;
            let expected: BTreeMap<_, _> = supplied
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            assert_eq!(
                primary_headers(&wire),
                expected,
                "{protocol:?}: SDK credentials must not appear without a resolved primary"
            );
        }
    }
}

#[tokio::test]
async fn explicit_empty_and_api_key_credentials_keep_presence_and_protocol_mapping() {
    for protocol in [
        RigProtocol::Chat,
        RigProtocol::Anthropic,
        RigProtocol::Responses,
    ] {
        for supplied in [
            vec![("api-key", "real-model-key")],
            vec![("api-key", "")],
            vec![("authorization", "")],
            vec![("authorization", "Bearer model-key"), ("x-api-key", "")],
        ] {
            let (wire, _, _) = support::capture_with_auth(
                &support::request(vec![support::user()]),
                protocol,
                Arc::new(HeaderAuth(header_map(&supplied))),
            )
            .await;
            let mut expected: BTreeMap<String, String> = supplied
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            if let Some(key) = expected.get("api-key").cloned() {
                match protocol {
                    RigProtocol::Chat => {
                        expected.insert(
                            "authorization".into(),
                            format!("Bearer {key}").trim_end().to_string(),
                        );
                    }
                    RigProtocol::Anthropic => {
                        expected.insert("x-api-key".into(), key);
                    }
                    RigProtocol::Responses => {}
                }
            }
            assert_eq!(
                primary_headers(&wire),
                expected,
                "{protocol:?}: explicit empty values are credentials chosen by the caller"
            );
        }
    }
}

#[tokio::test]
async fn unsupported_sdk_key_value_fails_before_http_without_exposing_secret_bytes() {
    for (protocol, name) in [
        (RigProtocol::Chat, "api-key"),
        (RigProtocol::Responses, "api-key"),
        (RigProtocol::Anthropic, "x-api-key"),
        (RigProtocol::Anthropic, "api-key"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback");
        let provider = super::error_tests::provider(listener.local_addr().expect("address"));
        let mut headers = HeaderMap::new();
        headers.insert(
            http::HeaderName::from_static(name),
            HeaderValue::from_bytes(b"\xffprivate-secret").expect("opaque header"),
        );
        let auth: SharedAuthProvider = Arc::new(HeaderAuth(headers));
        let result = stream_via_rig(
            &support::request(vec![support::user()]),
            &provider,
            &auth,
            HeaderMap::new(),
            protocol,
            Duration::from_secs(1),
        )
        .await;
        let error = match result {
            Ok(_) => panic!("invalid SDK key reached the stream"),
            Err(error) => error,
        };
        let expected =
            format!("Invalid {name} header: the Rig SDK requires an ASCII credential value");
        assert!(
            matches!(error, ApiError::InvalidRequest { message } if message == expected && !message.contains("private-secret"))
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "no request may be sent"
        );
    }
}
