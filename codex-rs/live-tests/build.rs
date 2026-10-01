//! Build receipt for live-test artifacts: binds the compiled test binary to
//! the source revision and target it was built from, so a run's manifest can
//! prove (or refute) that the executed binary matches the reported HEAD.
//!
//! The runtime manifest re-reads git at execution time; without this binding
//! a stale binary under a moved HEAD was indistinguishable from a fresh one.

fn main() {
    // Rebuild when the checked-out commit changes so the receipt stays
    // honest across commits.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=LIVE_TESTS_BUILD_GIT_SHA={sha}");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=LIVE_TESTS_BUILD_TARGET={target}");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=LIVE_TESTS_BUILD_PROFILE={profile}");
}
