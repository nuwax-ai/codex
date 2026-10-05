use super::super::*;
use super::common::*;
use pretty_assertions::assert_eq;
use sha2::Digest;

#[tokio::test]
async fn actual_scene_runner_matrix_retains_each_failure_after_home_cleanup() {
    for (kind, step, fault, expected) in [
        (Kind::Marker, 1, Fault::Launch, "injected launch failure"),
        (Kind::Marker, 1, Fault::Exit, "injected exit failure"),
        (
            Kind::Marker,
            1,
            Fault::MissingFinal,
            "produced no final answer file",
        ),
        (
            Kind::Marker,
            1,
            Fault::MissingMarker,
            "marker in its output",
        ),
        (Kind::Compact, 1, Fault::Launch, "injected launch failure"),
        (Kind::Compact, 2, Fault::Exit, "injected exit failure"),
        (
            Kind::Compact,
            2,
            Fault::MissingFinal,
            "produced no final answer file",
        ),
        (Kind::Compact, 2, Fault::WrongPassphrase, "passphrase lost"),
        (Kind::Compact, 2, Fault::NoCompaction, "compacted record"),
        (Kind::WebSearch, 1, Fault::Launch, "injected launch failure"),
        (Kind::WebSearch, 2, Fault::Exit, "injected exit failure"),
        (
            Kind::WebSearch,
            2,
            Fault::MissingFinal,
            "produced no final answer file",
        ),
        (Kind::WebSearch, 2, Fault::NoSearch, "matching call/result"),
        (
            Kind::Marker,
            1,
            Fault::ReplaceBinary,
            "changed after its receipt",
        ),
        (Kind::Marker, 1, Fault::PipeFailure, "injected pipe failure"),
        (Kind::Marker, 1, Fault::Timeout, "did not finish within"),
        (Kind::Marker, 1, Fault::WaitFailure, "injected wait failure"),
        (
            Kind::Marker,
            1,
            Fault::CaptureFailure,
            "output capture failed: injected capture failure",
        ),
    ] {
        let fixture = Fixture::new();
        let error = fixture
            .run(kind, Some((step, fault)))
            .await
            .expect_err("injected scene must fail");
        assert!(
            format!("{error:#}").contains(expected),
            "{fault:?}: {error:#}"
        );
        let original = std::fs::read(rollout_path(fixture.home.path())).expect("partial rollout");
        fixture.home.close().expect("cleanup temporary home");
        assert_eq!(
            std::fs::read(fixture.artifacts.path().join("rollout").join(ROLLOUT_NAME))
                .expect("retained"),
            original
        );
        if fault == Fault::Exit {
            let prefix = if step == 2 { "turn2." } else { "" };
            let events = std::fs::read(
                fixture
                    .artifacts
                    .path()
                    .join(format!("{prefix}events.jsonl")),
            )
            .expect("captured bytes");
            assert!(events.ends_with(b"\xffpartial"));
        }
    }
}

#[tokio::test]
async fn retention_and_capture_io_errors_preserve_the_original_scene_error() {
    let fixture = Fixture::new();
    std::fs::write(fixture.artifacts.path().join("rollout"), "conflict")
        .expect("retention conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::MissingMarker)))
        .await
        .expect_err("marker failure");
    assert_eq!(
        error.to_string(),
        "executed command must complete with exit 0 and the marker in its output"
    );
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.artifacts.path().join("events.jsonl")).expect("capture conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::Exit)))
        .await
        .expect_err("exit failure");
    assert_eq!(
        error.to_string(),
        "[offline]  exited with injected exit failure"
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

#[tokio::test]
async fn all_three_actual_scene_success_paths_use_prepared_binary() {
    for kind in [Kind::Marker, Kind::Compact, Kind::WebSearch] {
        Fixture::new()
            .run(kind, None)
            .await
            .expect("offline scene succeeds");
    }
}

#[tokio::test]
async fn changed_prepared_binary_fails_before_any_runner_call() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.prepared.path, "changed before turn").expect("replacement");
    let error = fixture
        .run(Kind::Marker, None)
        .await
        .expect_err("identity failure");
    assert!(error.to_string().contains("changed after its receipt"));
    assert!(!fixture.home.path().join("last_message.txt").exists());
    assert!(!rollout_path(fixture.home.path()).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn capture_pipe_bounds_descendant_drain_and_retains_partial_output() {
    let temp = tempfile::tempdir().expect("descendant directory");
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
        .expect("child");
    let pipe = child.stdout.take().expect("stdout");
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let reader = tokio::spawn(runner::capture_pipe(
        pipe,
        stopped,
        Duration::from_millis(30),
    ));
    assert!(child.wait().await.expect("exit").success());
    stop.send(true).expect("stop reader");
    let captured = tokio::time::timeout(Duration::from_secs(1), reader)
        .await
        .expect("bounded drain")
        .expect("reader");
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

#[cfg(unix)]
#[tokio::test]
async fn fake_executable_uses_real_process_capture_and_scene_retention() {
    use std::os::unix::fs::PermissionsExt;
    for exit_code in [0, 7] {
        let mut fixture = Fixture::new();
        let script = format!(
            r#"#!/bin/sh
last=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--output-last-message' ]; then
    shift
    last="$1"
  fi
  shift
done
mkdir -p "$CODEX_HOME/sessions/2026/10/03"
printf '%s\n' '{{"type":"session_meta"}}' > "$CODEX_HOME/sessions/2026/10/03/{ROLLOUT_NAME}"
printf '%s\n' 'done' > "$last"
printf '%s\n' '{{"type":"item.completed","item":{{"type":"command_execution","exit_code":0,"aggregated_output":"{MARKER}"}}}}'
printf '%s' 'dispatch-log' >&2
exit {exit_code}
"#
        );
        std::fs::write(&fixture.prepared.path, &script).expect("fake executable");
        std::fs::set_permissions(
            &fixture.prepared.path,
            std::fs::Permissions::from_mode(0o700),
        )
        .expect("executable permissions");
        fixture.prepared.sha256 = format!("{:x}", sha2::Sha256::digest(script.as_bytes()));
        let outcome = scenarios::run(
            &runner::ProcessRunner,
            &Scene {
                prepared: &fixture.prepared,
                home: fixture.home.path(),
                cwd: fixture.cwd.path(),
                artifacts: fixture.artifacts.path(),
                protocol: "subprocess",
                marker: MARKER,
                expected_model: "test-model",
                expected_url_prefix: "https://unit.test/v1",
                wire: codex_rust_rig_bridge::RigProtocol::Responses,
                capture_requirement: capture_validation::CaptureRequirement::Optional,
                expected_cap: capture_validation::CapExpectation::Absent,
            },
            BinaryScenario::Marker {
                expect_bridge_log: Some("dispatch-log"),
            },
        )
        .await;
        if exit_code == 0 {
            outcome.expect("real process success");
            assert!(fixture.artifacts.path().join("events.jsonl").is_file());
        } else {
            assert!(
                outcome
                    .expect_err("real process exit failure")
                    .to_string()
                    .contains("exited with")
            );
            fixture.home.close().expect("remove original home");
            assert_eq!(
                std::fs::read(fixture.artifacts.path().join("rollout").join(ROLLOUT_NAME))
                    .expect("retained bytes"),
                b"{\"type\":\"session_meta\"}\n"
            );
        }
    }
}
