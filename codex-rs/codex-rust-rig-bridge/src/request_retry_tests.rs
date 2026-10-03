use super::classify;
use rig_core::http_client::Error;

#[test]
fn local_instance_and_request_build_errors_are_not_http_retry_candidates() {
    let local = Error::Instance(Box::new(std::io::Error::other("capture failure")));
    assert!(classify(&local).is_none());
    let client = reqwest_rig::Client::new();
    let invalid = client
        .get("not a URL")
        .build()
        .expect_err("invalid request");
    assert!(classify(&Error::Instance(Box::new(invalid))).is_none());
}
