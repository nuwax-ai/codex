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
