use super::super::*;
use super::common::*;
use pretty_assertions::assert_eq;

#[test]
fn retention_accepts_historical_v1_v2_and_current_identity_with_actual_counts() {
    let home = tempfile::tempdir().expect("home");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let records: Vec<_> = [1, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(ordinal, version)| search_record(version, ordinal + 1).to_string())
        .collect();
    // Repeating a durable item does not count as another executed search.
    let bytes = format!("{}\n{}\n", records.join("\n"), records[2]);
    write_rollout(home.path(), bytes.as_bytes());
    assert_eq!(
        rollouts::retain_search_rollouts(home.path(), artifacts.path()).expect("valid versions"),
        rollouts::SearchEvidence {
            completed_pairs: 3,
            citations: 3
        }
    );
    home.close().expect("cleanup home");
    assert_eq!(
        std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("retained bytes"),
        bytes.into_bytes()
    );
}

#[test]
fn completed_status_alone_and_corrupt_identity_fail_after_retaining_bytes() {
    let valid = search_record(3, 1);
    let mut cases = Vec::new();
    for (field, value) in [
        ("version", serde_json::json!(99)),
        ("source", serde_json::json!("")),
        ("response_id", serde_json::json!("")),
        ("block_indices", serde_json::json!([0])),
        ("block_indices", serde_json::json!([0, 0])),
        ("block_indices", serde_json::json!([1, 0])),
    ] {
        let mut record = valid.clone();
        record["payload"]["wire_blocks"][field] = value;
        cases.push(record);
    }
    let mut wrong_owner = valid.clone();
    wrong_owner["payload"]["wire_blocks"]["layout"][2]["owner"] =
        serde_json::json!("rigseg_Another_0");
    cases.push(wrong_owner);
    let mut orphan_index = valid.clone();
    orphan_index["payload"]["wire_blocks"]["layout"][1]["index"] = serde_json::json!(9);
    orphan_index["payload"]["wire_blocks"]["layout"][2]["index"] = serde_json::json!(10);
    cases.push(orphan_index);
    let mut unmatched = valid.clone();
    unmatched["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] = serde_json::json!("other");
    cases.push(unmatched);
    let mut missing = valid;
    missing["payload"]
        .as_object_mut()
        .expect("payload")
        .remove("wire_blocks");
    cases.push(missing);
    for record in cases {
        let home = tempfile::tempdir().expect("home");
        let artifacts = tempfile::tempdir().expect("artifacts");
        let bytes = format!("{record}\n");
        write_rollout(home.path(), bytes.as_bytes());
        assert!(
            rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err(),
            "{record}"
        );
        home.close().expect("cleanup home");
        assert_eq!(
            std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("retained"),
            bytes.into_bytes()
        );
    }
}

#[test]
fn retention_preserves_truncated_non_utf8_bytes_before_parse_failure() {
    let home = tempfile::tempdir().expect("home");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let bytes = b"{\"type\":\"response_item\"\n\xff";
    write_rollout(home.path(), bytes);
    assert!(rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err());
    home.close().expect("cleanup home");
    assert_eq!(
        std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("retained"),
        bytes
    );
}
