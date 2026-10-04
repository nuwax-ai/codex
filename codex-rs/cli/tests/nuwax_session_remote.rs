//! Session commands must reject local provider credentials before connecting remotely.

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::Output;
use std::time::Duration;

const THREAD_ID: &str = "123e4567-e89b-12d3-a456-426614174000";
const REMOTE_PROVIDER_ERROR: &str = "NUWAX environment provider must be configured on the remote app-server host; unset the local NUWAX provider group or omit --remote";

#[tokio::test]
async fn session_commands_reject_active_nuwax_group_before_remote_connection() -> Result<()> {
    for wire_api in ["responses", "chat", "anthropic"] {
        for subcommand in ["queue", "archive", "unarchive", "delete"] {
            let output = run_session_command(subcommand, Some(wire_api)).await?;
            let stderr = String::from_utf8(output.stderr)?;
            assert!(
                !output.status.success(),
                "{subcommand}/{wire_api}: {stderr}"
            );
            assert!(
                stderr.contains(REMOTE_PROVIDER_ERROR),
                "{subcommand}/{wire_api}: {stderr}"
            );
            assert!(
                !stderr.contains("test-private-key"),
                "credentials in error output"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn session_commands_reject_incomplete_nuwax_group_before_remote_connection() -> Result<()> {
    for subcommand in ["queue", "archive", "unarchive", "delete"] {
        let output = run_session_command(subcommand, /*wire_api*/ None).await?;
        let stderr = String::from_utf8(output.stderr)?;
        assert!(!output.status.success(), "{subcommand}: {stderr}");
        assert!(
            stderr.contains("Error parsing NUWAX_* environment"),
            "{subcommand}: {stderr}"
        );
        assert!(
            !stderr.contains(REMOTE_PROVIDER_ERROR),
            "{subcommand}: {stderr}"
        );
    }
    Ok(())
}

async fn run_session_command(subcommand: &str, wire_api: Option<&str>) -> Result<Output> {
    let home = tempfile::tempdir()?;
    // A listening socket lets us detect even a short-lived connection that
    // would otherwise be hidden by a connection-refused error.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let remote = format!("ws://{}", listener.local_addr()?);
    let mut command = tokio::process::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    // Mutate only the child's environment, preserving concurrent test runners.
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|name| name.starts_with("NUWAX_")) {
            command.env_remove(name);
        }
    }
    command
        .current_dir(home.path())
        .env("CODEX_HOME", home.path())
        .env("NUWAX_BASE_URL", "https://client.example/v1")
        .arg(subcommand)
        .args(["--remote", &remote])
        .kill_on_drop(true);
    if let Some(wire_api) = wire_api {
        command
            .env("NUWAX_MODEL", "client-model")
            .env("NUWAX_WIRE_API", wire_api)
            .env("NUWAX_API_KEY", "test-private-key");
    }
    if subcommand == "queue" {
        command.args(["--thread", THREAD_ID, "--message", "do the thing"]);
    } else {
        command.arg(THREAD_ID);
        if subcommand == "delete" {
            command.arg("--force");
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .context("session command did not fail before remote connection")??;
    let error = listener
        .accept()
        .err()
        .context("session command connected before rejecting local provider environment")?;
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    Ok(output)
}
