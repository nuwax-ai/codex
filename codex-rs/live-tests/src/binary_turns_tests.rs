//! Parser tests for the binary-level rollout assertions.

use super::*;

fn write_rollout(home: &Path, lines: &[&str]) {
    let sessions = home.join("sessions/2026/09/30");
    std::fs::create_dir_all(&sessions).expect("session dir");
    std::fs::write(
        sessions.join("rollout-file-2026-09-30T00-00-00.jsonl"),
        lines.join("\n") + "\n",
    )
    .expect("rollout");
}

#[test]
fn completed_web_search_calls_counts_only_completed_items() {
    let home = tempfile::TempDir::new().expect("tempdir");
    write_rollout(
        home.path(),
        &[
            r#"{"type":"session_meta"}"#,
            r#"{"type":"response_item","payload":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"北京天气"}}}"#,
            r#"{"type":"response_item","payload":{"type":"web_search_call","id":"ws_2","status":"in_progress","action":{"type":"search","query":"ignored"}}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[]}}"#,
            r#"{"type":"turn_context"}"#,
        ],
    );
    assert_eq!(completed_web_search_calls(home.path()), 1);
}

#[test]
fn completed_web_search_calls_sums_across_rollout_files() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let search =
        r#"{"type":"response_item","payload":{"type":"web_search_call","status":"completed"}}"#;
    write_rollout(home.path(), &[search, r#"{"type":"compacted"}"#, search]);
    // A second rollout under the same home contributes as well.
    let other = home.path().join("sessions/2026/09/29");
    std::fs::create_dir_all(&other).expect("dir");
    std::fs::write(
        other.join("rollout-file-2026-09-29T00-00-00.jsonl"),
        format!("{search}\n"),
    )
    .expect("rollout");
    assert_eq!(completed_web_search_calls(home.path()), 3);
}

#[test]
fn completed_web_search_calls_on_an_empty_home_is_zero() {
    let home = tempfile::TempDir::new().expect("tempdir");
    assert_eq!(completed_web_search_calls(home.path()), 0);
}

const SEARCH_ENVELOPE: &str = r#"{"type":"response_item","payload":{"type":"web_search_call","status":"completed","wire_blocks":{"version":1,"source":"anthropic:test-source","blocks":[{"type":"server_tool_use","id":"srv_1","name":"web_search"},{"type":"web_search_tool_result","tool_use_id":"srv_1","content":[]}]}}}"#;

#[test]
fn retain_search_rollouts_copies_and_validates_records() {
    let home = tempfile::TempDir::new().expect("home");
    let artifacts = tempfile::TempDir::new().expect("artifacts");
    write_rollout(home.path(), &[SEARCH_ENVELOPE]);
    retain_search_rollouts(home.path(), artifacts.path()).expect("retained envelope");
    let file = std::fs::read_dir(artifacts.path())
        .expect("dir")
        .next()
        .expect("file")
        .expect("entry");
    assert_eq!(
        std::fs::read_to_string(file.path()).expect("retained"),
        format!("{SEARCH_ENVELOPE}\n")
    );
}

#[test]
fn retain_search_rollouts_rejects_unrelated_or_invalid_envelopes() {
    let valid: Value = serde_json::from_str(SEARCH_ENVELOPE).expect("search record");
    let mut cases = Vec::new();
    let mut unrelated = valid.clone();
    unrelated["payload"]["type"] = serde_json::json!("message");
    cases.push(unrelated);
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("source", serde_json::json!("")),
        ("blocks", serde_json::json!({})),
        ("blocks", serde_json::json!([])),
    ] {
        let mut record = valid.clone();
        record["payload"]["wire_blocks"][field] = value;
        cases.push(record);
    }
    for record in cases {
        let home = tempfile::TempDir::new().expect("home");
        let artifacts = tempfile::TempDir::new().expect("artifacts");
        write_rollout(home.path(), &[&record.to_string()]);
        assert!(
            retain_search_rollouts(home.path(), artifacts.path()).is_err(),
            "{record}"
        );
    }
    let home = tempfile::TempDir::new().expect("home");
    let artifacts = tempfile::TempDir::new().expect("artifacts");
    write_rollout(home.path(), &["not JSON", SEARCH_ENVELOPE]);
    assert!(retain_search_rollouts(home.path(), artifacts.path()).is_err());
}

#[test]
fn retain_search_rollouts_propagates_copy_failure() {
    let home = tempfile::TempDir::new().expect("home");
    let artifacts = tempfile::TempDir::new().expect("artifacts");
    write_rollout(home.path(), &[SEARCH_ENVELOPE]);
    std::fs::create_dir(
        artifacts
            .path()
            .join("rollout-file-2026-09-30T00-00-00.jsonl"),
    )
    .expect("conflicting directory");
    let error = retain_search_rollouts(home.path(), artifacts.path()).expect_err("copy must fail");
    assert!(error.to_string().contains("retain rollout"));
}

#[test]
fn failed_rollout_retention_preserves_truncated_bytes_after_home_cleanup() {
    let home = tempfile::TempDir::new().expect("home");
    let artifacts = tempfile::TempDir::new().expect("artifacts");
    let partial = format!("{SEARCH_ENVELOPE}\n{{\"type\":\"response_item\"");
    write_rollout(home.path(), &[&partial]);
    retain_rollouts_best_effort(home.path(), artifacts.path()).expect("retain partial rollout");
    home.close().expect("remove temporary home");
    let retained = artifacts
        .path()
        .join("rollout-file-2026-09-30T00-00-00.jsonl");
    assert_eq!(
        std::fs::read_to_string(retained).expect("retained"),
        format!("{partial}\n")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn capture_pipe_bounds_descendant_drain_and_retains_partial_output() {
    let temp = tempfile::tempdir().expect("descendant completion directory");
    let finished = temp.path().join("finished");
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "printf partial; (sleep 2; : > \"$1\") &",
            "capture-helper",
        ])
        .arg(&finished)
        .stdout(Stdio::piped())
        .spawn()
        .expect("pipe-holding child");
    let pipe = child.stdout.take().expect("stdout");
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let reader = tokio::spawn(capture_pipe(pipe, stopped, Duration::from_millis(30)));
    assert!(child.wait().await.expect("child exit").success());
    stop.send(true).expect("stop reader");
    let captured = tokio::time::timeout(Duration::from_secs(1), reader)
        .await
        .expect("bounded drain")
        .expect("reader task");
    // Drain must finish early, but test teardown also waits for the owned
    // descendant to exit rather than leaving nextest's stderr pipe open.
    tokio::time::timeout(Duration::from_secs(4), async {
        while !finished.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("descendant exited");
    assert_eq!(
        (captured.bytes, captured.error.is_some()),
        (b"partial".to_vec(), true)
    );
}
