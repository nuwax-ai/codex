use super::*;
use pretty_assertions::assert_eq;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn detached_children_do_not_capture_client_model_seeds() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let completed = temp.path().join("completed");
    // Supply ambient values in a subprocess without mutating this test process.
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "backend::pid::start::tests::launch_with_client_model_seeds",
            "--ignored",
        ])
        .env("CODEX_MODEL_REASONING_EFFORT", "none")
        .env("CODEX_MODEL_CONTEXT_WINDOW", "60000")
        .env("CODEX_AUTO_COMPACT_TOKEN_LIMIT", "12000")
        .env("CODEX_AUTO_COMPACT_RATIO", "0.2")
        .env("NUWAX_MODEL", "client-model")
        .env("NUWAX_BASE_URL", "https://client.example/v1")
        .env("NUWAX_WIRE_API", "chat")
        .env("NUWAX_API_KEY", "client-secret")
        .env("NUWAX_MAX_OUTPUT_TOKENS", "2048")
        .env("NUWAX_REQUEST_MAX_RETRIES", "1")
        .env("NUWAX_STREAM_MAX_RETRIES", "2")
        .env("NUWAX_STREAM_IDLE_TIMEOUT_MS", "120000")
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
#[ignore = "subprocess entry point for detached_children_do_not_capture_client_model_seeds"]
async fn launch_with_client_model_seeds() {
    for name in [
        "CODEX_MODEL_REASONING_EFFORT",
        "CODEX_MODEL_CONTEXT_WINDOW",
        "CODEX_AUTO_COMPACT_TOKEN_LIMIT",
        "CODEX_AUTO_COMPACT_RATIO",
        "NUWAX_MODEL",
        "NUWAX_BASE_URL",
        "NUWAX_WIRE_API",
        "NUWAX_API_KEY",
        "NUWAX_MAX_OUTPUT_TOKENS",
        "NUWAX_REQUEST_MAX_RETRIES",
        "NUWAX_STREAM_MAX_RETRIES",
        "NUWAX_STREAM_IDLE_TIMEOUT_MS",
    ] {
        assert!(
            std::env::var_os(name).is_some_and(|value| !value.is_empty()),
            "subprocess must launch with {name} set"
        );
    }
    for kind in ["app-server", "updater"] {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let binary = temp.path().join("codex-shim");
        std::fs::write(
            &binary,
            b"#!/bin/sh\ncase \"$*\" in *--help*) exit 0 ;; esac\nprintf '%s\\n' \"${CODEX_MODEL_REASONING_EFFORT-unset}\" \"${CODEX_MODEL_CONTEXT_WINDOW-unset}\" \"${CODEX_AUTO_COMPACT_TOKEN_LIMIT-unset}\" \"${CODEX_AUTO_COMPACT_RATIO-unset}\" \"${NUWAX_MODEL-unset}\" \"${NUWAX_BASE_URL-unset}\" \"${NUWAX_WIRE_API-unset}\" \"${NUWAX_API_KEY-unset}\" \"${NUWAX_MAX_OUTPUT_TOKENS-unset}\" \"${NUWAX_REQUEST_MAX_RETRIES-unset}\" \"${NUWAX_STREAM_MAX_RETRIES-unset}\" \"${NUWAX_STREAM_IDLE_TIMEOUT_MS-unset}\" > \"$0.env\"\nexec sleep 30\n",
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
                    Ok(value) if value.lines().count() == 12 => break Ok(value),
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => break Err(error),
                }
                tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
            }
        })
        .await;
        backend.stop().await.expect("stop detached child");
        let observed = observed
            .expect("child recorded environment")
            .expect("read child environment");
        assert_eq!(
            observed.lines().collect::<Vec<_>>(),
            ["unset"; 12],
            "{kind} must not inherit any client model seed"
        );
    }
    std::fs::write(
        std::env::var_os("CODEX_TEST_DAEMON_EFFORT_COMPLETE").expect("completion path"),
        "app-server,updater",
    )
    .expect("write completion marker");
}

#[test]
fn two_clients_with_different_temporary_credentials_do_not_leak_into_daemon_children() {
    // Two clients started with different NUWAX_* credentials each exercise
    // the real detached-start path; neither client's secrets may appear in
    // any daemon-spawned child environment, regardless of value.
    for (index, (model, secret)) in [
        ("client-model-a", "client-secret-a"),
        ("client-model-b", "client-secret-b"),
    ]
    .into_iter()
    .enumerate()
    {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let completed = temp.path().join(format!("completed-{index}"));
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "backend::pid::start::tests::launch_with_client_model_seeds",
                "--ignored",
            ])
            .env("CODEX_MODEL_REASONING_EFFORT", "none")
            .env("CODEX_MODEL_CONTEXT_WINDOW", "60000")
            .env("CODEX_AUTO_COMPACT_TOKEN_LIMIT", "12000")
            .env("CODEX_AUTO_COMPACT_RATIO", "0.2")
            .env("NUWAX_MODEL", model)
            .env("NUWAX_BASE_URL", "https://client.example/v1")
            .env("NUWAX_WIRE_API", "chat")
            .env("NUWAX_API_KEY", secret)
            .env("NUWAX_MAX_OUTPUT_TOKENS", "2048")
            .env("NUWAX_REQUEST_MAX_RETRIES", "1")
            .env("NUWAX_STREAM_MAX_RETRIES", "2")
            .env("NUWAX_STREAM_IDLE_TIMEOUT_MS", "120000")
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
            std::fs::read_to_string(&completed).expect("subprocess exercised both launch paths"),
            "app-server,updater"
        );
    }
}
