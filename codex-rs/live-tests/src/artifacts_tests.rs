use super::*;

fn expected() -> ReceiptExpectation<'static> {
    ReceiptExpectation {
        git_sha: "0123456789012345678901234567890123456789",
        source_sha256: "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
        target: "x86_64-unknown-linux-gnu",
        profile: "debug",
        package_features: &[],
    }
}

fn receipt() -> serde_json::Value {
    let expected = expected();
    serde_json::json!({
        "format": "codex-exec-build-receipt", "version": 1,
        "git_sha": expected.git_sha, "git_dirty": "clean",
        "source": {"algorithm": source_identity::SOURCE_ALGORITHM, "sha256": expected.source_sha256},
        "target": expected.target, "profile": expected.profile,
        "features": {"scope": "codex-exec", "enabled": []},
        "dependency_features": {"status": "unknown", "reason": "cargo_build_script_has_no_resolved_dependency_feature_graph"},
    })
}

#[test]
fn receipt_validates_source_and_package_without_claiming_dependency_graph() {
    for dirty in ["clean", "dirty"] {
        let mut receipt = receipt();
        receipt["git_dirty"] = dirty.into();
        let entry = validate_receipt(receipt.to_string().as_bytes(), Some(&expected()));
        assert_eq!(
            (
                entry["status"].as_str(),
                entry["source_validated"].as_bool(),
                entry["package_configuration_validated"].as_bool(),
                entry["build_configuration_validated"].as_bool()
            ),
            (
                Some("source_and_package_validated"),
                Some(true),
                Some(true),
                Some(false)
            )
        );
    }
}

#[test]
fn receipt_rejects_missing_fields_invalid_json_and_unknown_schema_fields() {
    let complete = receipt();
    for field in [
        "format",
        "version",
        "git_sha",
        "git_dirty",
        "source",
        "target",
        "profile",
        "features",
        "dependency_features",
    ] {
        let mut incomplete = complete.clone();
        incomplete.as_object_mut().expect("object").remove(field);
        let entry = validate_receipt(incomplete.to_string().as_bytes(), Some(&expected()));
        assert_eq!(entry["status"], "invalid", "missing {field}");
        assert_eq!(entry["source_validated"], false);
    }
    let mut extra = complete;
    extra["claims_signed"] = true.into();
    for invalid in [
        b"not JSON".to_vec(),
        extra.to_string().into_bytes(),
        b"{\"format\":\"a\",\"format\":\"b\"}".to_vec(),
    ] {
        assert_eq!(
            validate_receipt(&invalid, Some(&expected()))["status"],
            "invalid"
        );
    }
}

#[test]
fn receipt_rejects_wrong_revision_content_target_profile_and_features() {
    for (field, value, source_validated, package_validated) in [
        (
            "git_sha",
            serde_json::json!("ffffffffffffffffffffffffffffffffffffffff"),
            false,
            true,
        ),
        (
            "source",
            serde_json::json!({"algorithm": source_identity::SOURCE_ALGORITHM, "sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"}),
            false,
            true,
        ),
        (
            "target",
            serde_json::json!("aarch64-apple-darwin"),
            true,
            false,
        ),
        ("profile", serde_json::json!("release"), true, false),
        (
            "features",
            serde_json::json!({"scope":"codex-exec", "enabled":["unexpected"]}),
            true,
            false,
        ),
    ] {
        let mut mismatch = receipt();
        mismatch[field] = value;
        let entry = validate_receipt(mismatch.to_string().as_bytes(), Some(&expected()));
        assert_eq!(
            (
                entry["status"].as_str(),
                entry["source_validated"].as_bool(),
                entry["package_configuration_validated"].as_bool()
            ),
            (
                Some("mismatch"),
                Some(source_validated),
                Some(package_validated)
            ),
            "{field}"
        );
    }
    let mut short = receipt();
    short["git_sha"] = "0123456".into();
    assert_eq!(
        validate_receipt(short.to_string().as_bytes(), Some(&expected()))["status"],
        "invalid"
    );
}

#[test]
fn receipt_cannot_validate_without_git_and_source_expectation() {
    let entry = validate_receipt(receipt().to_string().as_bytes(), None);
    assert_eq!(
        (
            entry["status"].as_str(),
            entry["source_validated"].as_bool()
        ),
        (Some("unresolved"), Some(false))
    );
    let mut unavailable = receipt();
    unavailable["git_sha"] = "unknown".into();
    unavailable["source"]["sha256"] = "unknown".into();
    let entry = validate_receipt(unavailable.to_string().as_bytes(), Some(&expected()));
    assert_eq!(
        (
            entry["status"].as_str(),
            entry["source_validated"].as_bool()
        ),
        (Some("unresolved"), Some(false))
    );
}

#[test]
fn content_identity_detects_same_path_edits_and_new_source_ignoring_runtime_files() {
    let root = tempfile::TempDir::new().expect("source tree");
    let root = root.path();
    std::fs::create_dir_all(root.join("codex-rs/exec/src")).expect("source directory");
    let source = root.join("codex-rs/exec/src/main.rs");
    std::fs::write(&source, "fn main() { /* first */ }\n").expect("source");
    for args in [
        vec!["init"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Receipt Test",
            "-c",
            "user.email=receipt@example.invalid",
            "commit",
            "--no-gpg-sign",
            "-m",
            "source",
        ],
    ] {
        source_identity::git_output(root, &args).expect("prepare git tree");
    }
    let clean = source_identity::source_identity(root).expect("clean identity");
    assert!(!clean.dirty);
    std::fs::write(&source, "fn main() { /* second */ }\n").expect("first edit");
    let first = source_identity::source_identity(root).expect("first identity");
    let first_status =
        source_identity::git_output(root, &["status", "--porcelain"]).expect("status");
    std::fs::write(&source, "fn main() { /* third */ }\n").expect("same path edit");
    let second = source_identity::source_identity(root).expect("second identity");
    assert_eq!(
        source_identity::git_output(root, &["status", "--porcelain"]).expect("status"),
        first_status
    );
    assert_ne!(clean.fingerprint, first.fingerprint);
    assert_ne!(first.fingerprint, second.fingerprint);
    std::fs::write(root.join("codex-rs/exec/src/new.rs"), "pub fn added() {}\n")
        .expect("new untracked implementation");
    let added = source_identity::source_identity(root).expect("added identity");
    assert_ne!(second.fingerprint, added.fingerprint);
    std::fs::write(root.join("codex-rs/state_5.sqlite"), "runtime").expect("runtime database");
    std::fs::create_dir(root.join("codex-rs/tmp")).expect("runtime temp directory");
    std::fs::write(root.join("codex-rs/tmp/process.txt"), "runtime").expect("runtime temp file");
    std::fs::write(root.join("codex-rs/.env.local"), "secret").expect("credential file");
    assert_eq!(
        source_identity::source_identity(root)
            .expect("runtime identity")
            .fingerprint,
        added.fingerprint
    );
    std::fs::remove_file(&source).expect("delete source");
    assert_ne!(
        source_identity::source_identity(root)
            .expect("deletion identity")
            .fingerprint,
        added.fingerprint
    );
}

#[test]
fn prepared_exec_rejects_changes_to_the_same_file() {
    let dir = tempfile::TempDir::new().expect("directory");
    let path = dir.path().join("exec");
    std::fs::write(&path, b"first binary").expect("binary");
    let prepared = PreparedExec {
        sha256: binary_sha256(&path).expect("hash"),
        path,
    };
    prepared.verify_unchanged().expect("unchanged");
    std::fs::write(&prepared.path, b"later binary").expect("replace binary");
    assert!(prepared.verify_unchanged().is_err());
}

#[test]
fn executable_hash_rejects_nonregular_paths() {
    let directory = tempfile::TempDir::new().expect("directory");
    let error = binary_sha256(directory.path()).expect_err("reject directory");
    assert_eq!(
        error.to_string(),
        "codex-exec hash input must be a regular file"
    );
}

#[cfg(unix)]
#[test]
fn receipt_process_enforces_stdout_stderr_and_time_limits() {
    for (script, stdout_limit, stderr_limit) in
        [("printf 12345", 4, 32), ("printf 12345 >&2", 32, 4)]
    {
        let result = source_identity::bounded_output(
            std::process::Command::new("sh").args(["-c", script]),
            Duration::from_secs(1),
            stdout_limit,
            stderr_limit,
        );
        assert!(result.is_err(), "{script}");
    }
    let started = std::time::Instant::now();
    let result = source_identity::bounded_output(
        std::process::Command::new("sh").args(["-c", "exec sleep 2"]),
        Duration::from_millis(50),
        32,
        32,
    );
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}
