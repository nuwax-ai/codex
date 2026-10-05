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

/// A complete group whose WIRE_API value is garbage is a corrupted group:
/// the command must fail fast naming the variable, before any remote
/// connection, and without echoing credentials.
#[tokio::test]
async fn session_commands_reject_corrupted_nuwax_group_values_before_remote_connection()
-> Result<()> {
    let home = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let remote = format!("ws://{}", listener.local_addr()?);
    let mut command = tokio::process::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|name| name.starts_with("NUWAX_")) {
            command.env_remove(name);
        }
    }
    command
        .current_dir(home.path())
        .env("CODEX_HOME", home.path())
        .env("NUWAX_BASE_URL", "https://client.example/v1")
        .env("NUWAX_WIRE_API", "carrier-pigeon")
        .env("NUWAX_API_KEY", "corrupted-group-key")
        .env("NUWAX_MODEL", "client-model")
        .arg("archive")
        .args(["--remote", &remote])
        .arg(THREAD_ID)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .context("session command did not fail on the corrupted group")??;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("NUWAX_WIRE_API"),
        "the error must name the corrupted variable: {stderr}"
    );
    assert!(
        !stderr.contains(REMOTE_PROVIDER_ERROR),
        "a corrupted group is a parse failure, not the remote-provider policy: {stderr}"
    );
    assert!(
        !stderr.contains("corrupted-group-key"),
        "values must never be echoed: {stderr}"
    );
    let error = listener
        .accept()
        .err()
        .context("session command connected before rejecting the corrupted group")?;
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    Ok(())
}

/// With NO NUWAX environment at all the group is inactive: session commands
/// pass the local-credential policy and actually reach the remote endpoint
/// (the raw listener observes the connection, then closes it).
#[tokio::test]
async fn session_commands_without_nuwax_group_reach_the_remote_connection() -> Result<()> {
    let home = tempfile::tempdir()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let remote = format!("ws://{}", listener.local_addr()?);
    let mut command = tokio::process::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|name| name.starts_with("NUWAX_")) {
            command.env_remove(name);
        }
    }
    command
        .current_dir(home.path())
        .env("CODEX_HOME", home.path())
        .arg("archive")
        .args(["--remote", &remote])
        .arg(THREAD_ID)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    // The command must get past every local env policy and open the socket.
    let (stream, _peer) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
        .await
        .context("inactive group must still reach the remote connection")??;
    // Dropping the accepted socket forces the client to fail its handshake
    // instead of hanging until the outer timeout.
    drop(stream);
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .context("session command should exit after the remote drops the connection")??;
    assert!(
        !output.status.success(),
        "the raw listener cannot complete a handshake; the command must fail"
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        !stderr.contains(REMOTE_PROVIDER_ERROR),
        "no NUWAX group is present, the remote-provider policy must not fire: {stderr}"
    );
    Ok(())
}
