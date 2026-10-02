//! Revision receipt for the live-test harness, not the separate codex-exec
//! binary launched by binary-level tests.

fn git_output(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    // HEAD usually contains a branch name, whose ref changes on commit.
    // Ask Git for paths so worktree .git files and shared refs also work.
    let branch = git_output(&["symbolic-ref", "--quiet", "HEAD"]);
    for reference in [Some("HEAD"), branch.as_deref(), Some("packed-refs")]
        .into_iter()
        .flatten()
    {
        if let Some(path) = git_output(&["rev-parse", "--git-path", reference]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let sha = git_output(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=LIVE_TESTS_BUILD_GIT_SHA={sha}");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=LIVE_TESTS_BUILD_TARGET={target}");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=LIVE_TESTS_BUILD_PROFILE={profile}");
}
