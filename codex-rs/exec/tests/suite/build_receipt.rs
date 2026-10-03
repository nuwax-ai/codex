//! The receipt command runs without agent machinery and rejects other arguments.

use codex_utils_cargo_bin::cargo_bin;
use pretty_assertions::assert_eq;
use serde_json::Value;

#[test]
fn build_receipt_flag_prints_versioned_source_and_package_identity() {
    let output = std::process::Command::new(cargo_bin("codex-exec").expect("resolve codex-exec"))
        .arg("--build-receipt")
        .output()
        .expect("spawn codex-exec");
    assert!(output.status.success(), "status: {:?}", output.status);
    let receipt: Value = serde_json::from_slice(&output.stdout).expect("receipt JSON");
    assert_eq!(receipt["format"], "codex-exec-build-receipt");
    assert_eq!(receipt["version"], 1);
    for field in ["git_sha", "git_dirty", "target", "profile"] {
        assert!(
            receipt[field].is_string(),
            "{field} must be present: {receipt}"
        );
    }
    // Bazel/release archives may have no Git metadata. Such builds remain
    // explicitly unresolved rather than inventing a source identity.
    let revision = receipt["git_sha"].as_str().expect("revision");
    assert!(
        revision == "unknown"
            || (matches!(revision.len(), 40 | 64)
                && revision.bytes().all(|byte| byte.is_ascii_hexdigit()))
    );
    let fingerprint = receipt["source"]["sha256"]
        .as_str()
        .expect("source fingerprint");
    assert!(
        fingerprint == "unknown"
            || (fingerprint.len() == 64
                && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()))
    );
    assert_eq!(
        receipt["source"]["algorithm"],
        "git-build-input-content-sha256-v1"
    );
    assert!(matches!(
        receipt["git_dirty"].as_str(),
        Some("clean" | "dirty" | "unknown")
    ));
    assert_eq!(receipt["features"]["scope"], "codex-exec");
    assert!(receipt["features"]["enabled"].is_array());
    assert_eq!(
        receipt["dependency_features"],
        serde_json::json!({
            "status": "unknown", "reason": "cargo_build_script_has_no_resolved_dependency_feature_graph",
        })
    );
}

#[test]
fn build_receipt_flag_rejects_trailing_options_and_prompts() {
    for trailing in ["--unknown-option", "prompt"] {
        let output =
            std::process::Command::new(cargo_bin("codex-exec").expect("resolve codex-exec"))
                .args(["--build-receipt", trailing])
                .output()
                .expect("spawn codex-exec");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be used alone"));
    }
}

#[cfg(unix)]
#[test]
fn build_receipt_flag_handles_non_unicode_argument_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let output = std::process::Command::new(cargo_bin("codex-exec").expect("resolve codex-exec"))
        .arg("--build-receipt")
        .arg(std::ffi::OsString::from_vec(vec![0xff]))
        .output()
        .expect("spawn codex-exec");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must be used alone"));
    assert!(!stderr.contains("panicked"));
}
