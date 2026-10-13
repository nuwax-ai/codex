use super::*;
use pretty_assertions::assert_eq;

fn temporary_git_tree() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "core.quotepath", "false"],
        vec!["config", "core.longpaths", "true"],
    ] {
        git_output(root.path(), &args).unwrap();
    }
    root
}

#[test]
fn tree_status_preserves_short_entries_without_reporting_truncation() {
    let root = temporary_git_tree();
    for name in ["first file.rs", "日本語.rs"] {
        std::fs::write(root.path().join(name), "source").unwrap();
    }
    let raw =
        String::from_utf8(git_output(root.path(), &["status", "--porcelain"]).unwrap()).unwrap();
    let status: serde_json::Value =
        serde_json::from_str(&tree_status(root.path()).unwrap()).unwrap();
    assert_eq!(
        status,
        serde_json::json!({
            "entries": 2,
            "head": raw.lines().collect::<Vec<_>>(),
            "truncated_entries": 0,
            "omitted_entries": 0,
        })
    );
}

#[test]
fn diagnostic_prefixes_end_at_complete_json_escapes_and_unicode_characters() {
    for padding in 0..8 {
        let original = format!("{}{}", " ".repeat(padding), "\u{1}\"\\界".repeat(/*n*/ 80));
        let (escaped, truncated) = json_escape_bounded(&original);
        assert!(truncated);
        assert!(escaped.len() <= MAX_ESCAPED_STATUS_ENTRY_BYTES);
        let decoded: String = serde_json::from_str(&format!("\"{escaped}\"")).unwrap();
        assert!(original.starts_with(decoded.strip_suffix("...").unwrap()));
    }
}

#[test]
fn long_unicode_and_quoted_status_paths_keep_the_complete_receipt_bounded() {
    let root = temporary_git_tree();
    let mut directory = root.path().to_path_buf();
    for index in 0..4 {
        directory.push(format!("{index}-{}", "界".repeat(/*n*/ 60)));
        std::fs::create_dir(&directory).unwrap();
    }
    for index in 0..40 {
        #[cfg(unix)]
        let name = format!("{index:02}-{}-日本語.rs", "\"".repeat(/*n*/ 90));
        #[cfg(not(unix))]
        let name = format!("{index:02}-{}-日本語.rs", " ".repeat(/*n*/ 30));
        std::fs::write(directory.join(name), "source").unwrap();
    }
    // Staged files are individually listed instead of collapsing an untracked
    // directory to one short status entry. No repository-wide environment is
    // changed, including Git's global quote-path or long-path settings.
    git_output(root.path(), &["add", "."]).unwrap();
    let raw =
        String::from_utf8(git_output(root.path(), &["status", "--porcelain"]).unwrap()).unwrap();
    #[cfg(unix)]
    assert!(
        serde_json::to_vec(
            &raw.lines()
                .take(MAX_TREE_STATUS_ENTRIES)
                .collect::<Vec<_>>()
        )
        .unwrap()
        .len()
            > 32 * 1024,
        "the original uncapped status head would exceed the receipt limit"
    );
    let encoded = tree_status(root.path()).unwrap();
    assert!(encoded.len() <= MAX_TREE_STATUS_BYTES);
    let status: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let head = status["head"].as_array().unwrap();
    assert_eq!(head.len(), MAX_TREE_STATUS_ENTRIES);
    assert_eq!(
        (
            status["entries"].as_u64(),
            status["truncated_entries"].as_u64(),
            status["omitted_entries"].as_u64(),
        ),
        (Some(40), Some(32), Some(8))
    );
    for (original, entry) in raw.lines().zip(head) {
        let prefix = entry.as_str().unwrap().strip_suffix("...").unwrap();
        assert!(original.starts_with(prefix));
        assert!(serde_json::to_string(entry).unwrap().len() <= MAX_ESCAPED_STATUS_ENTRY_BYTES + 2);
    }
    let receipt = serde_json::json!({
        "format": "codex-exec-build-receipt",
        "version": 1,
        "git_sha": "a".repeat(/*n*/ 40),
        "git_dirty": "dirty",
        "source": {"algorithm": SOURCE_ALGORITHM, "sha256": "b".repeat(/*n*/ 64)},
        "tree_status": status,
        "target": "x86_64-unknown-linux-gnu",
        "profile": "debug",
        "features": {"scope": "codex-exec", "enabled": []},
        "dependency_features": {
            "status": "unknown",
            "reason": "cargo_build_script_has_no_resolved_dependency_feature_graph",
        },
    });
    assert!(serde_json::to_vec(&receipt).unwrap().len() <= 32 * 1024);
}

#[test]
fn exhausted_pipe_budget_prevents_spawning_another_probe_and_releases_reservations() {
    let leases: Vec<_> = (0..MAX_PIPE_READERS)
        .map(|_| PipeReaderLease::reserve().unwrap())
        .collect();
    let error = bounded_output(
        &mut Command::new("this-command-must-never-be-spawned"),
        Duration::from_secs(1),
        1024,
        1024,
    )
    .unwrap_err();
    assert!(error.contains("pipe reader budget exhausted"));
    drop(leases);
    assert!(PipeReaderLease::reserve().is_ok());
}

#[cfg(unix)]
#[test]
fn descendant_owned_pipes_time_out_without_unbounded_reader_growth() {
    for _ in 0..MAX_PIPE_READERS / 2 {
        let error = bounded_output(
            Command::new("sh").args(["-c", "sleep 3 & exit 0"]),
            Duration::from_millis(20),
            1024,
            1024,
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
    }
    let error = bounded_output(
        &mut Command::new("this-command-must-never-be-spawned"),
        Duration::from_secs(1),
        1024,
        1024,
    )
    .unwrap_err();
    assert!(error.contains("pipe reader budget exhausted"));
}
