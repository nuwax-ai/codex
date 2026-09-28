//! Hermetic tests for the fork default home resolution. The user home is a
//! temp dir, so results never depend on the developer machine's
//! `~/.codex-nuwax` state.

use super::find_default_codex_home;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use tempfile::TempDir;

fn absolute(path: &Path) -> AbsolutePathBuf {
    AbsolutePathBuf::from_absolute_path(path).expect("absolute temp path")
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create symlink");
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_dir(target, link).expect("create dir symlink");
}

#[test]
fn without_nuwax_home_falls_back_to_codex_dir() {
    let user_home = TempDir::new().expect("temp user home");

    let resolved = find_default_codex_home(user_home.path()).expect("default home");

    assert_eq!(resolved, absolute(&user_home.path().join(".codex")));
}

#[test]
fn nuwax_home_directory_is_selected_without_canonicalization() {
    let user_home = TempDir::new().expect("temp user home");
    let nuwax_home = user_home.path().join(".codex-nuwax");
    fs::create_dir(&nuwax_home).expect("create nuwax home");

    let resolved = find_default_codex_home(user_home.path()).expect("default home");

    assert_eq!(resolved, absolute(&nuwax_home));
}

#[test]
fn nuwax_home_regular_file_is_fatal() {
    let user_home = TempDir::new().expect("temp user home");
    let nuwax_home = user_home.path().join(".codex-nuwax");
    fs::write(&nuwax_home, "not a directory").expect("write regular file");

    let err = find_default_codex_home(user_home.path()).expect_err("file nuwax home");

    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains(".codex-nuwax"),
        "unexpected error: {err}"
    );
}

#[test]
fn nuwax_home_wins_over_codex_dir() {
    let user_home = TempDir::new().expect("temp user home");
    fs::create_dir(user_home.path().join(".codex")).expect("create codex dir");
    let nuwax_home = user_home.path().join(".codex-nuwax");
    fs::create_dir(&nuwax_home).expect("create nuwax home");

    let resolved = find_default_codex_home(user_home.path()).expect("default home");

    assert_eq!(resolved, absolute(&nuwax_home));
}

#[test]
fn nuwax_home_symlink_to_directory_is_selected_as_the_link_path() {
    let user_home = TempDir::new().expect("temp user home");
    let target = user_home.path().join("actual-home");
    fs::create_dir(&target).expect("create symlink target dir");
    let nuwax_home = user_home.path().join(".codex-nuwax");
    create_dir_symlink(&target, &nuwax_home);

    let resolved = find_default_codex_home(user_home.path()).expect("default home");

    assert_eq!(resolved, absolute(&nuwax_home));
}

#[test]
fn nuwax_home_dangling_symlink_is_fatal() {
    let user_home = TempDir::new().expect("temp user home");
    let nuwax_home = user_home.path().join(".codex-nuwax");
    create_dir_symlink(&user_home.path().join("missing-target"), &nuwax_home);

    let err = find_default_codex_home(user_home.path()).expect_err("dangling nuwax home");

    assert!(
        err.to_string().contains(".codex-nuwax"),
        "unexpected error: {err}"
    );
}
