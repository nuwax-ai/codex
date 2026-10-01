use super::redacted_endpoint;
use pretty_assertions::assert_eq;

#[test]
fn endpoint_redaction_removes_userinfo_paths_and_query_credentials() {
    for (input, expected) in [
        (
            "https://user:secret@gateway.example:8443/token?api_key=secret#private",
            "https://gateway.example:8443",
        ),
        (
            "https://secret@gateway.example/v1",
            "https://gateway.example",
        ),
        ("http://user:secret@[::1]:8080/v1", "http://[::1]:8080"),
        ("invalid-secret", "<unparsed>"),
    ] {
        assert_eq!(redacted_endpoint(Some(input)), expected);
    }
}
