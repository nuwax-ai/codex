use super::classify;
use rig_core::http_client::Error;

#[test]
fn local_and_request_build_errors_are_not_http_retry_candidates() {
    let local = Error::permanent(std::io::Error::other("capture failure"));
    assert!(classify(&local).is_none());
    let client = reqwest_rig::Client::new();
    let invalid = client
        .get("not a URL")
        .build()
        .expect_err("invalid request");
    assert!(classify(&Error::permanent(invalid)).is_none());
}

/// rig 0.47 lets the transport say whether sending again can work. The same
/// connection failure is worth another attempt only while the transport
/// called it `Transient`; once it is `Permanent`, it fails at once.
#[tokio::test]
async fn only_a_transient_transport_failure_is_a_retry_candidate() {
    let client = reqwest_rig::Client::builder().no_proxy().build().unwrap();
    // Nothing listens on port 1: binding it needs privileges the test lacks.
    // `reqwest::Error` is not `Clone`: take one failure per verdict.
    let mut refused = Vec::new();
    for _ in 0..2 {
        let failure = client
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("the connection must be refused");
        assert!(
            failure.is_connect() || failure.is_request(),
            "fixture must be a connection failure: {failure}"
        );
        refused.push(failure);
    }
    let mut refused = refused.into_iter();
    let transient = refused.next().unwrap();
    let permanent = refused.next().unwrap();
    assert!(
        matches!(
            classify(&Error::transient(transient)),
            Some(codex_api::TransportError::Network(_))
        ),
        "the transport's own transient failure is worth another attempt"
    );
    assert!(
        classify(&Error::permanent(permanent)).is_none(),
        "a permanent verdict is never second-guessed"
    );
}
