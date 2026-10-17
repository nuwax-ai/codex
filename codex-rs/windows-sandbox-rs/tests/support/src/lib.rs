//! Coordinates tests that share the runner's Windows sandbox accounts across processes.

#![cfg(windows)]

use std::fs::File;
use std::fs::OpenOptions;
use std::io;

/// Serializes tests that provision or use the runner's shared Windows sandbox accounts.
/// Hold this from before setup until all sandbox commands and cleanup have finished.
#[must_use = "the account lock is released when the guard is dropped"]
pub struct WindowsSandboxAccountTestGuard {
    _file: File,
}

impl WindowsSandboxAccountTestGuard {
    /// Acquires the same file lock across all test processes run by this Windows user.
    pub fn acquire() -> io::Result<Self> {
        stage_command_runner()?;
        let directory = dirs::data_local_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Windows local application data directory is unavailable",
            )
        })?;
        // Bazel uses per-test temporary directories, so the lock lives in the runner user's
        // Windows known folder instead. It is separate from the production setup lock.
        let path = directory.join("codex-windows-sandbox-integration-test.lock");
        // Leave the file in place: deleting it would let processes lock different files at this path.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("open Windows sandbox test lock: {error}"),
                )
            })?;
        file.lock().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("lock Windows sandbox test accounts: {error}"),
            )
        })?;
        Ok(Self { _file: file })
    }
}

/// Stages `codex-command-runner.exe` next to the running test binary.
///
/// Elevated sandbox sessions resolve the command runner from the test
/// executable's directory; a bare `cargo test` run never places it there.
/// Copy it from the shared cargo target profile directory when the test
/// harness layout is detected, and fail with a build instruction otherwise —
/// without this the spawn surfaces as the misleading OS error 2 from
/// CreateProcessWithLogonW.
fn stage_command_runner() -> io::Result<()> {
    const RUNNER: &str = "codex-command-runner.exe";
    let test_exe = std::env::current_exe()?;
    let Some(deps_dir) = test_exe
        .parent()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "deps"))
    else {
        // Non-cargo layouts (bazel sandboxes) stage their own helpers.
        return Ok(());
    };
    let staged = deps_dir.join(RUNNER);
    if staged.is_file() {
        return Ok(());
    }
    let Some(profile_dir) = deps_dir.parent() else {
        return Ok(());
    };
    let built = profile_dir.join(RUNNER);
    if built.is_file() {
        std::fs::copy(&built, &staged).map(|_| ()).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("stage {RUNNER} for sandbox tests: {error}"),
            )
        })?;
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{RUNNER} is missing for elevated sandbox tests; build it first with \
             `cargo build -p codex-windows-sandbox --bin codex-command-runner` \
             (expected {} or {})",
            built.display(),
            staged.display()
        ),
    ))
}
