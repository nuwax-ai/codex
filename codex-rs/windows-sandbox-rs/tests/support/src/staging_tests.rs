use super::stage_command_runner_for_exe;
use std::fs;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;

struct Profile(PathBuf);

impl Profile {
    fn new() -> io::Result<Self> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-runner-staging-{}-{nonce}",
            std::process::id(),
        ));
        fs::create_dir_all(root.join("deps"))?;
        Ok(Self(root))
    }

    fn test_exe(&self) -> PathBuf {
        self.0.join("deps/sandbox-test.exe")
    }

    fn built(&self) -> PathBuf {
        self.0.join("codex-command-runner.exe")
    }

    fn staged(&self) -> PathBuf {
        self.0.join("deps/codex-command-runner.exe")
    }

    fn close(self) -> io::Result<()> {
        fs::remove_dir_all(&self.0)
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        // Explicit close reports cleanup errors on success; this also cleans
        // up a fixture when an assertion or an earlier filesystem call fails.
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn staged_runner_refreshes_after_a_same_length_profile_rebuild() -> io::Result<()> {
    let profile = Profile::new()?;
    fs::write(profile.built(), b"old runner")?;
    stage_command_runner_for_exe(&profile.test_exe())?;
    fs::write(profile.built(), b"new runner")?;

    stage_command_runner_for_exe(&profile.test_exe())?;

    assert_eq!(fs::read(profile.staged())?, b"new runner");
    profile.close()
}

#[test]
fn identical_staged_runner_is_reused_without_write_sharing() -> io::Result<()> {
    const FILE_SHARE_READ: u32 = 1;
    let profile = Profile::new()?;
    fs::write(profile.built(), b"same runner")?;
    fs::write(profile.staged(), b"same runner")?;
    // Deny writes/deletes like an executing image: reading identical contents
    // is sufficient, and trying to overwrite this file must fail on Windows.
    let in_use = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(profile.staged())?;

    stage_command_runner_for_exe(&profile.test_exe())?;
    drop(in_use);
    profile.close()
}

#[test]
fn explicitly_staged_layout_does_not_require_a_profile_copy() -> io::Result<()> {
    let profile = Profile::new()?;
    fs::write(profile.staged(), b"pre-staged runner")?;

    stage_command_runner_for_exe(&profile.test_exe())?;
    profile.close()
}
