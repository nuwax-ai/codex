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
