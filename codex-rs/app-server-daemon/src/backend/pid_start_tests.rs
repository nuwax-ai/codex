use super::*;
use pretty_assertions::assert_eq;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn detached_children_do_not_capture_client_effort() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let completed = temp.path().join("completed");
    // Supply an ambient value in a subprocess without mutating this test process.
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "backend::pid::start::tests::launch_with_client_effort",
            "--ignored",
        ])
        .env("CODEX_MODEL_REASONING_EFFORT", "none")
        .env("CODEX_TEST_DAEMON_EFFORT_COMPLETE", &completed)
        .output()
        .expect("run isolated launcher");
    assert!(
        output.status.success(),
        "launcher failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(completed).expect("subprocess exercised both launch paths"),
        "app-server,updater"
    );
}

#[tokio::test]
#[ignore = "subprocess entry point for detached_children_do_not_capture_client_effort"]
async fn launch_with_client_effort() {
    assert_eq!(
        std::env::var("CODEX_MODEL_REASONING_EFFORT").unwrap(),
        "none"
    );
    for kind in ["app-server", "updater"] {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let binary = temp.path().join("codex-shim");
        std::fs::write(
            &binary,
            b"#!/bin/sh\ncase \"$*\" in *--help*) exit 0 ;; esac\nprintf '%s' \"${CODEX_MODEL_REASONING_EFFORT-unset}\" > \"$0.env\"\nexec sleep 30\n",
        )
        .expect("write shim");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(/*mode*/ 0o755))
            .expect("executable shim");
        let pid_file = temp.path().join("state/child.pid");
        let backend = if kind == "app-server" {
            PidBackend::new(
                binary.clone(),
                pid_file,
                /*remote_control_enabled*/ false,
            )
        } else {
            PidBackend::new_update_loop(binary.clone(), pid_file, /*restore_release*/ None)
        };
        backend.start().await.expect("start detached child");
        let observed = tokio::time::timeout(Duration::from_secs(/*secs*/ 3), async {
            loop {
                match fs::read_to_string(binary.with_extension("env")).await {
                    Ok(value) if !value.is_empty() => break Ok(value),
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => break Err(error),
                }
                tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
            }
        })
        .await;
        backend.stop().await.expect("stop detached child");
        assert_eq!(
            observed
                .expect("child recorded environment")
                .expect("read child environment"),
            "unset",
            "{kind}"
        );
    }
    std::fs::write(
        std::env::var_os("CODEX_TEST_DAEMON_EFFORT_COMPLETE").expect("completion path"),
        "app-server,updater",
    )
    .expect("write completion marker");
}
