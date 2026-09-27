use super::*;
use serde_json::json;

#[test]
fn fixture_root_override_keeps_both_recordings_together() {
    const CHILD_ROOT: &str = "CODEX_CASSETTE_TEST_ROOT";
    const VERIFIED: &str = "cassette fixture paths verified";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        assert_eq!(
            (
                fixture_path("vendor", "rig", "turn"),
                rig_event_fixture_path("vendor", "turn"),
            ),
            (
                Some(root.join("vendor/rig-turn.json")),
                Some(root.join("vendor/rig-events-turn.json")),
            )
        );
        println!("{VERIFIED}");
        return;
    }

    // A child process isolates environment-dependent path selection from
    // concurrent tests and from a developer's recording configuration.
    let root = tempfile::tempdir().expect("temporary fixture root");
    for custom_root in [Some(root.path().to_path_buf()), None] {
        let expected_root = custom_root.clone().unwrap_or_else(|| {
            repo_root()
                .expect("repository root")
                .join("codex-rs/live-tests/tests/fixtures")
        });
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "cassette::tests::fixture_root_override_keeps_both_recordings_together",
                "--nocapture",
            ])
            .env(CHILD_ROOT, expected_root);
        if let Some(custom_root) = custom_root {
            command.env("LIVE_FIXTURE_DIR", custom_root);
        } else {
            command.env_remove("LIVE_FIXTURE_DIR");
        }
        let output = command.output().expect("run isolated fixture path test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(VERIFIED),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

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
