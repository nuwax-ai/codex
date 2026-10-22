use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn immutable_bearer_snapshot_matches_actual_resolved_headers() {
    let provider = BearerAuthProvider::new("private-test-key".into());
    assert_eq!(
        provider.immutable_credential_headers(),
        Some(provider.resolve_auth_headers().await.unwrap())
    );
}

#[tokio::test]
async fn malformed_bearer_fails_before_send_without_revealing_key() {
    let provider = BearerAuthProvider::new("private-secret\r\nInjected: secret".into());
    assert_eq!(provider.immutable_credential_headers(), None);
    let error = provider
        .resolve_auth_headers()
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "request auth build error: invalid bearer authentication header"
    );
    assert!(!error.contains("private-secret"));
}

#[tokio::test]
async fn malformed_static_credentials_fail_sync_and_async_without_partial_headers() {
    for provider in [
        BearerAuthProvider::new("private-secret\r\nInjected: secret".into()),
        BearerAuthProvider::for_test(
            Some("private-secret"),
            Some("workspace\r\nInjected: secret"),
        ),
    ] {
        let expected = HeaderMap::from_iter([(
            http::header::USER_AGENT,
            HeaderValue::from_static("existing-client"),
        )]);
        let mut headers = expected.clone();
        let sync_error = provider.add_auth_headers(&mut headers).unwrap_err();
        let async_error = provider.resolve_auth_headers().await.unwrap_err();
        assert_eq!(headers, expected);
        assert_eq!(sync_error.to_string(), async_error.to_string());
        assert!(!sync_error.to_string().contains("private-secret"));
    }
}
