use super::super::*;
use super::common::*;
use pretty_assertions::assert_eq;
use sha2::Digest;

#[tokio::test]
async fn uncited_search_answer_records_zero_citations_and_keeps_basic_search_success() {
    let fixture = Fixture::new();
    fixture
        .run(Kind::WebSearch, Some((2, Fault::NoCitations)))
        .await
        .expect("legitimate uncited answer");
    let evidence: Value = serde_json::from_slice(
        &std::fs::read(fixture.artifacts.path().join("search-evidence.json")).expect("evidence"),
    )
    .expect("evidence JSON");
    assert_eq!(
        evidence,
        serde_json::json!({
            "turn1": {"answer_chars":16,"completed_matched_pairs":1,"citations":1},
            "turn2": {"answer_chars":16,"completed_matched_pairs":1,"citations":0},
            "citation_capability_asserted":false,
        })
    );
}

#[tokio::test]
async fn request_capture_presence_and_failures_are_recorded_without_masking_scene_failure() {
    let fixture = Fixture::new();
    fixture
        .run(Kind::Marker, None)
        .await
        .expect("uncaptured scene");
    let read_status = |fixture: &Fixture| -> Value {
        serde_json::from_slice(
            &std::fs::read(
                fixture
                    .artifacts
                    .path()
                    .join("request-capture-evidence.json"),
            )
            .expect("capture evidence"),
        )
        .expect("capture evidence JSON")
    };
    assert_eq!(
        read_status(&fixture),
        serde_json::json!({
            "read_status":"absent","http_attempts":0,"wire_asserted":false,
            "asserted_fields":[],
            "path":"requests.jsonl",
        })
    );
    let fixture = Fixture::new();
    std::fs::write(
        fixture.artifacts.path().join("requests.jsonl"),
        "{\"method\":\"POST\",\"url\":\"https://unit.test/v1/responses\",\"body_raw\":\"{\\\"model\\\":\\\"test-model\\\"}\",\"body\":{\"model\":\"test-model\"}}\n",
    )
    .expect("trace");
    fixture
        .run(Kind::Marker, None)
        .await
        .expect("captured scene");
    assert_eq!(
        read_status(&fixture),
        serde_json::json!({
            "read_status":"available","http_attempts":1,"wire_asserted":true,
            "asserted_fields":["url.scheme","url.authority","url.path","body.model","body.max_output_tokens_absent"],
            "path":"requests.jsonl",
        })
    );
    let fixture = Fixture::new();
    std::fs::write(
        fixture.artifacts.path().join("requests.jsonl"),
        "truncated JSON",
    )
    .expect("malformed capture");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::MissingMarker)))
        .await
        .expect_err("scene failure");
    assert_eq!(
        error.to_string(),
        "executed command must complete with exit 0 and the marker in its output"
    );
    assert_eq!(
        read_status(&fixture)["read_status"],
        serde_json::json!("malformed")
    );
    assert!(
        fixture
            .artifacts
            .path()
            .join("rollout")
            .join(ROLLOUT_NAME)
            .is_file()
    );
}

#[test]
fn glm_prime_search_query_and_string_result_remain_verbatim_matched_evidence() {
    let home = tempfile::tempdir().expect("home");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let mut record = search_record(3, 1);
    let blocks = &mut record["payload"]["wire_blocks"]["blocks"];
    blocks[0]["name"] = serde_json::json!("web_search_prime");
    blocks[0]["input"] = serde_json::json!({
        "location":"cn", "search_query":"北京 今天 天气 2026年10月3日", "search_recency_filter":"oneDay",
    });
    blocks[1] = serde_json::json!({"type":"tool_result", "tool_use_id":"srv_1", "content":"gateway result"});
    let bytes = format!("{record}\n");
    write_rollout(home.path(), bytes.as_bytes());
    assert_eq!(
        rollouts::retain_search_rollouts(home.path(), artifacts.path())
            .expect("actual gateway shapes"),
        rollouts::SearchEvidence {
            completed_pairs: 1,
            citations: 1
        }
    );
    assert_eq!(
        std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("verbatim retained bytes"),
        bytes.as_bytes()
    );
    // Accepting the gateway name cannot relax the call/result identity contract.
    record["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] =
        serde_json::json!("different-call");
    write_rollout(home.path(), format!("{record}\n").as_bytes());
    assert!(rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err());
    record["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] = serde_json::json!("srv_1");
    record["payload"]["wire_blocks"]["blocks"][0]["name"] = serde_json::json!("unrelated-tool");
    write_rollout(home.path(), format!("{record}\n").as_bytes());
    assert!(rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err());
}

#[tokio::test]
async fn post_execution_hash_failure_retains_received_bytes_and_wins_over_capture_io_error() {
    let fixture = Fixture::new();
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::ReplaceBinary)))
        .await
        .expect_err("hash must fail");
    assert!(error.to_string().contains("changed after its receipt"));
    let events: Value = serde_json::from_slice(
        &std::fs::read(fixture.artifacts.path().join("events.jsonl"))
            .expect("captured stdout before final verification"),
    )
    .expect("event JSON");
    assert_eq!(
        events,
        serde_json::json!({"type":"item.completed","item":{
            "type":"command_execution", "exit_code":0, "aggregated_output":MARKER,
        }})
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log")).expect("stderr retained"),
        b"dispatch-log"
    );
    let exit = std::fs::read_to_string(fixture.artifacts.path().join("exit.txt"))
        .expect("exit diagnostic");
    assert!(exit.starts_with("exit 0\n"));
    assert!(exit.contains("executable identity check failed"));
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.artifacts.path().join("events.jsonl"))
        .expect("capture I/O conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::ReplaceBinary)))
        .await
        .expect_err("hash remains primary");
    assert!(error.to_string().contains("changed after its receipt"));
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log"))
            .expect("remaining captures still written"),
        b"dispatch-log"
    );
}

#[tokio::test]
async fn capture_pipe_keeps_exact_limit_and_reports_overflow_with_the_complete_allowed_prefix() {
    let (_stop, stopped) = tokio::sync::watch::channel(/*init*/ false);
    let exact = vec![b'x'; runner::MAX_PIPE_BYTES];
    let capture = runner::capture_pipe(
        exact.as_slice(),
        stopped.clone(),
        Duration::from_secs(/*secs*/ 1),
    )
    .await;
    assert_eq!((capture.bytes, capture.error), (exact.clone(), None));
    let mut oversized = exact.clone();
    oversized.extend_from_slice(b"overflow");
    let capture = runner::capture_pipe(
        oversized.as_slice(),
        stopped,
        Duration::from_secs(/*secs*/ 1),
    )
    .await;
    assert_eq!(
        (capture.bytes, capture.error),
        (
            exact,
            Some("pipe exceeded 16 MiB capture limit".to_string())
        )
    );
}

#[tokio::test]
async fn capture_failure_remains_primary_when_artifact_io_also_fails() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.artifacts.path().join("events.jsonl"))
        .expect("capture I/O conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::CaptureFailure)))
        .await
        .expect_err("capture failure");
    assert_eq!(
        error.to_string(),
        "[offline]  output capture failed: injected capture failure"
    );
    assert!(fixture.artifacts.path().join("stderr.log").is_file());
    assert!(
        fixture
            .artifacts
            .path()
            .join("rollout")
            .join(ROLLOUT_NAME)
            .is_file()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn real_process_output_overflow_stops_the_child_before_its_wait_deadline_and_retains_both_pipes()
 {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let size = runner::MAX_PIPE_BYTES + 1;
    let script = format!(
        "#!/bin/sh\nprintf '%s' 'stderr prefix' >&2\nhead -c {size} /dev/zero\nexec sleep 60\n"
    );
    std::fs::write(&fixture.prepared.path, &script).expect("fake executable");
    std::fs::set_permissions(
        &fixture.prepared.path,
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("executable");
    fixture.prepared.sha256 = format!("{:x}", sha2::Sha256::digest(script.as_bytes()));
    let outcome = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 10),
        scenarios::run(
            &runner::ProcessRunner,
            &Scene {
                prepared: &fixture.prepared,
                home: fixture.home.path(),
                cwd: fixture.cwd.path(),
                artifacts: fixture.artifacts.path(),
                protocol: "overflow",
                marker: MARKER,
                expected_model: "test-model",
                expected_url_prefix: "https://unit.test/v1",
                wire: codex_rust_rig_bridge::RigProtocol::Responses,
                capture_requirement: capture_validation::CaptureRequirement::Optional,
                expected_cap: capture_validation::CapExpectation::Absent,
            },
            BinaryScenario::Marker {
                expect_bridge_log: None,
            },
        ),
    )
    .await
    .expect("capture overflow must stop the child promptly");
    let error = outcome.expect_err("explicit capture failure");
    assert!(
        error
            .to_string()
            .contains("output capture failed: stdout: pipe exceeded 16 MiB capture limit")
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("events.jsonl"))
            .expect("retained stdout prefix"),
        vec![0; runner::MAX_PIPE_BYTES]
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log")).expect("drained stderr"),
        b"stderr prefix"
    );
    let exit =
        std::fs::read_to_string(fixture.artifacts.path().join("exit.txt")).expect("exit evidence");
    assert!(exit.starts_with("output capture failed:"));
    assert!(!exit.contains("timeout after"));
}

#[tokio::test]
async fn capture_pipe_read_failure_preserves_every_preceding_completed_read() {
    struct FailsAfterPrefix {
        prefix: Option<&'static [u8]>,
    }
    impl tokio::io::AsyncRead for FailsAfterPrefix {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut().prefix.take() {
                Some(prefix) => {
                    buffer.put_slice(prefix);
                    std::task::Poll::Ready(Ok(()))
                }
                None => {
                    std::task::Poll::Ready(Err(std::io::Error::other("injected pipe read failure")))
                }
            }
        }
    }
    let (_stop, stopped) = tokio::sync::watch::channel(/*init*/ false);
    let capture = runner::capture_pipe(
        FailsAfterPrefix {
            prefix: Some(b"complete prefix"),
        },
        stopped,
        Duration::from_secs(/*secs*/ 1),
    )
    .await;
    assert_eq!(
        (capture.bytes, capture.error),
        (
            b"complete prefix".to_vec(),
            Some("injected pipe read failure".to_string())
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn known_nonzero_child_exit_remains_primary_over_later_pipe_drain_failure() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let script = "#!/bin/sh\nprintf '%s' 'stdout prefix'\nprintf '%s' 'stderr prefix' >&2\n(sleep 6; : > \"$CODEX_HOME/descendant-finished\") &\nexit 7\n";
    std::fs::write(&fixture.prepared.path, script).expect("fake executable");
    std::fs::set_permissions(
        &fixture.prepared.path,
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("executable");
    fixture.prepared.sha256 = format!("{:x}", sha2::Sha256::digest(script.as_bytes()));
    let outcome = scenarios::run(
        &runner::ProcessRunner,
        &Scene {
            prepared: &fixture.prepared,
            home: fixture.home.path(),
            cwd: fixture.cwd.path(),
            artifacts: fixture.artifacts.path(),
            protocol: "late-drain",
            marker: MARKER,
            expected_model: "test-model",
            expected_url_prefix: "https://unit.test/v1",
            wire: codex_rust_rig_bridge::RigProtocol::Responses,
            capture_requirement: capture_validation::CaptureRequirement::Optional,
            expected_cap: capture_validation::CapExpectation::Absent,
        },
        BinaryScenario::Marker {
            expect_bridge_log: None,
        },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
        while !fixture.home.path().join("descendant-finished").exists() {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 20)).await;
        }
    })
    .await
    .expect("owned descendant teardown");
    let error = outcome.expect_err("natural exit must fail");
    assert!(error.to_string().starts_with("[late-drain]  exited with"));
    let exit =
        std::fs::read_to_string(fixture.artifacts.path().join("exit.txt")).expect("exit evidence");
    assert!(exit.contains("capture incomplete: pipe did not close"));
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("events.jsonl")).expect("stdout prefix"),
        b"stdout prefix"
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log")).expect("stderr prefix"),
        b"stderr prefix"
    );
}
