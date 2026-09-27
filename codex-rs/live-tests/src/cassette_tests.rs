use super::*;
use serde_json::json;

#[test]
fn fixture_write_round_trips_the_complete_record() {
    let root = tempfile::tempdir().expect("temporary fixture root");
    let path = root.path().join("vendor/rig-turn.json");
    let fixture = TurnFixture {
        vendor: "vendor".into(),
        bridge: "rig".into(),
        tag: "turn".into(),
        request: json!({"model":"fixture-model","input":[]}),
        events: vec![ResponseEvent::Created { response_id: None }],
    };
    write_fixture(&path, &fixture).expect("record fixture");
    let recorded: TurnFixture =
        serde_json::from_slice(&std::fs::read(&path).expect("read fixture"))
            .expect("decode fixture");
    assert_eq!(
        serde_json::to_value(recorded).expect("recorded JSON"),
        serde_json::to_value(fixture).expect("expected JSON")
    );
}

#[test]
fn fixture_directory_and_write_failures_include_the_affected_path() {
    let root = tempfile::tempdir().expect("temporary fixture root");
    let blocked_parent = root.path().join("file-instead-of-directory");
    std::fs::write(&blocked_parent, "existing file").expect("block directory creation");
    let error = write_fixture(&blocked_parent.join("turn.json"), &json!({}))
        .expect_err("directory failure must propagate");
    assert!(
        error
            .to_string()
            .contains(&blocked_parent.display().to_string())
    );

    let error = write_fixture(root.path(), &json!({}))
        .expect_err("writing a file over a directory must fail");
    assert!(
        error
            .to_string()
            .contains(&root.path().display().to_string())
    );
}

#[test]
fn serialization_failure_preserves_the_previous_fixture() {
    let root = tempfile::tempdir().expect("temporary fixture root");
    let path = root.path().join("turn.json");
    std::fs::write(&path, "previous fixture").expect("old fixture");
    let invalid = std::collections::BTreeMap::from([(vec![1, 2], "invalid JSON map key")]);
    let error = write_fixture(&path, &invalid).expect_err("serialization must fail");
    assert!(error.to_string().contains(&path.display().to_string()));
    assert_eq!(
        std::fs::read_to_string(path).expect("old fixture remains"),
        "previous fixture"
    );
}
