use super::support;
use codex_api::AuthProvider;
use codex_rust_rig_bridge::RigProtocol;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct VersionedAuth;

impl AuthProvider for VersionedAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        support::DummyAuth.add_auth_headers(headers);
        headers.insert("anthropic-version", HeaderValue::from_static("2025-01-01"));
    }
}

#[tokio::test]
async fn configured_anthropic_version_reaches_the_final_http_request() {
    let (wire, _, _) = support::capture_with_auth(
        &support::request(vec![support::user()]),
        RigProtocol::Anthropic,
        Arc::new(VersionedAuth),
    )
    .await;
    let versions: Vec<_> = wire["headers"]
        .as_str()
        .expect("captured HTTP headers")
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("anthropic-version")
                .then_some(value.trim())
        })
        .collect();
    assert_eq!(versions, vec!["2025-01-01"]);
}
