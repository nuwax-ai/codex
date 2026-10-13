use super::super::*;

#[test]
fn credential_scan_detects_secret_bytes_in_binary_files_under_every_root() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let cfg = LiveConfig {
        vendor: "scan-fixture".to_string(),
        api_key: "credential-scan-fixture-key".to_string(),
        base_url: "http://fixture.invalid".to_string(),
        anthropic_base_url: None,
        responses_base_url: None,
        model: "scan-fixture".to_string(),
    };
    for root in [home.path(), cwd.path(), artifacts.path()] {
        let path = root.join("binary.db");
        let mut bytes = vec![0xff; 8190];
        bytes.extend_from_slice(cfg.api_key.as_bytes());
        bytes.push(0xfe);
        std::fs::write(&path, bytes).expect("binary fixture");
        let result = assert_no_credential_leak(home.path(), cwd.path(), artifacts.path(), &cfg);
        assert!(
            result.is_err(),
            "binary credential must not be treated as clean"
        );
        std::fs::remove_file(path).expect("remove fixture");
    }
}

#[cfg(unix)]
#[test]
fn credential_scan_propagates_unreadable_file_errors() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let cfg = LiveConfig {
        vendor: "scan-fixture".to_string(),
        api_key: "credential-scan-fixture-key".to_string(),
        base_url: "http://fixture.invalid".to_string(),
        anthropic_base_url: None,
        responses_base_url: None,
        model: "scan-fixture".to_string(),
    };
    std::os::unix::fs::symlink("missing-file", artifacts.path().join("unreadable.log"))
        .expect("unreadable fixture");
    assert!(
        assert_no_credential_leak(home.path(), cwd.path(), artifacts.path(), &cfg).is_err(),
        "failed reads must not be treated as empty files"
    );
}

#[test]
fn credential_scan_detects_json_escaped_key_in_retained_artifacts() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let cfg = LiveConfig {
        vendor: "scan-fixture".to_string(),
        api_key: r#"quoted"key\suffix"#.to_string(),
        base_url: "http://fixture.invalid".to_string(),
        anthropic_base_url: None,
        responses_base_url: None,
        model: "scan-fixture".to_string(),
    };
    std::fs::write(
        artifacts.path().join("metadata.json"),
        serde_json::json!({"token": cfg.api_key}).to_string(),
    )
    .expect("escaped artifact");
    assert!(
        assert_no_credential_leak(home.path(), cwd.path(), artifacts.path(), &cfg).is_err(),
        "JSON string escaping must not hide a persisted credential"
    );
}
