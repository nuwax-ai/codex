//! Multiple denied files must not prevent sandbox startup or expose protected contents.

use super::LONG_TIMEOUT_MS;
use super::codex_linux_sandbox_exe;
use super::create_env_from_core_vars;
use super::run_cmd_result_with_permission_profile_for_cwd;
use super::should_skip_bwrap_tests;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cargo_bin::write_executable;
use pretty_assertions::assert_eq;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;

enum DeniedFileRules {
    ExactPaths,
    Globs,
    GlobsWithProtectedRg,
    GlobsWithNativeFallback,
}

#[test_case::test_case(false, false; "root_read")]
#[test_case::test_case(true, false; "explicit_device_read")]
#[test_case::test_case(false, true; "writable_shm")]
#[tokio::test]
async fn denied_dev_symlink_cannot_be_recreated_or_expose_host_ptys(
    read_dev: bool,
    write_shm: bool,
) {
    assert!(
        !should_skip_bwrap_tests().await,
        "precondition: this device-isolation contract requires a working bubblewrap sandbox"
    );
    let temp = tempfile::tempdir().expect("workspace");
    let workspace = AbsolutePathBuf::try_from(temp.path()).expect("absolute workspace");
    let helper = codex_linux_sandbox_exe();
    let helper_dir = AbsolutePathBuf::try_from(helper.parent().expect("helper directory"))
        .expect("absolute helper directory");
    let mut entries = vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: if read_dev {
                    FileSystemSpecialPath::Minimal
                } else {
                    FileSystemSpecialPath::Root
                },
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(helper_dir.into(), FileSystemAccessMode::Read),
        FileSystemSandboxEntry::new(workspace.clone().into(), FileSystemAccessMode::Read),
        FileSystemSandboxEntry::new(
            AbsolutePathBuf::from_absolute_path("/dev/fd")
                .expect("absolute denied device path")
                .into(),
            FileSystemAccessMode::Deny,
        ),
    ];
    if read_dev {
        entries.push(FileSystemSandboxEntry::new(
            AbsolutePathBuf::from_absolute_path("/dev")
                .expect("absolute device directory")
                .into(),
            FileSystemAccessMode::Read,
        ));
    }
    let shm_file = write_shm.then(|| {
        entries.push(FileSystemSandboxEntry::new(
            AbsolutePathBuf::from_absolute_path("/dev/shm")
                .expect("absolute shared-memory directory")
                .into(),
            FileSystemAccessMode::Write,
        ));
        tempfile::NamedTempFile::new_in("/dev/shm").expect("shared-memory fixture")
    });

    // Compare only a PTY owned by this test. Do not inspect any other terminal.
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: the fd output pointers are valid; null optional pointers request
    // the platform defaults. The successful descriptors are immediately owned.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(result, 0, "precondition: create a task-owned host PTY");
    // SAFETY: a successful openpty returned these two uniquely owned fds.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC)
            .expect("keep the host PTY sentinel out of the sandbox's inherited descriptors");
    }
    let sentinel = std::fs::read_link(format!("/proc/self/fd/{}", slave.as_raw_fd()))
        .expect("task-owned host PTY name");
    assert!(sentinel.exists(), "precondition: host PTY sentinel exists");
    let mut env = create_env_from_core_vars();
    env.insert("LC_ALL".to_string(), "C".to_string());
    env.insert(
        "CODEX_TEST_HOST_PTY_SENTINEL".to_string(),
        sentinel.to_str().expect("UTF-8 PTY name").to_string(),
    );
    if let Some(file) = &shm_file {
        env.insert(
            "CODEX_TEST_WRITABLE_SHM".to_string(),
            file.path().to_str().expect("UTF-8 shm path").to_string(),
        );
    }
    let output = run_cmd_result_with_permission_profile_for_cwd(
        &[
            "/bin/sh",
            "-c",
            r#"set -eu
test ! -e /dev/fd && test ! -L /dev/fd
if ln -s /proc/self/fd /dev/fd; then exit 73; fi
test ! -e /dev/fd && test ! -L /dev/fd
test ! -e "$CODEX_TEST_HOST_PTY_SENTINEL"
test ! -e /dev/ptmx && test ! -L /dev/ptmx
printf 'standard device works' > /dev/null
if test -n "${CODEX_TEST_WRITABLE_SHM:-}"; then
    printf 'writable shm survives' > "$CODEX_TEST_WRITABLE_SHM"
fi
printf 'restricted-dev-runtime-ok\n'
"#,
        ],
        workspace,
        PermissionProfile::from_runtime_permissions(
            &FileSystemSandboxPolicy::restricted(entries),
            NetworkSandboxPolicy::Enabled,
        ),
        env,
        LONG_TIMEOUT_MS,
        /*use_legacy_landlock*/ false,
    )
    .await
    .expect("the restricted device tree must still execute the command");
    assert_eq!(output.exit_code, 0, "sandbox output: {output:?}");
    assert_eq!(output.stdout.text, "restricted-dev-runtime-ok\n");
    assert!(output.stderr.text.contains("Read-only file system"));
    if let Some(file) = &shm_file {
        assert_eq!(
            std::fs::read_to_string(file.path()).expect("read shared-memory fixture"),
            "writable shm survives"
        );
    }
}

#[test_case::test_case(DeniedFileRules::ExactPaths; "exact_paths")]
#[test_case::test_case(DeniedFileRules::Globs; "globs")]
#[test_case::test_case(DeniedFileRules::GlobsWithProtectedRg; "globs_with_protected_rg")]
#[test_case::test_case(DeniedFileRules::GlobsWithNativeFallback; "globs_with_native_fallback")]
#[tokio::test]
async fn sandbox_starts_with_multiple_denied_files(rules: DeniedFileRules) {
    if should_skip_bwrap_tests().await {
        eprintln!("skipping bwrap test: bwrap sandbox prerequisites are unavailable");
        return;
    }

    let use_controlled_rg_path = matches!(
        rules,
        DeniedFileRules::GlobsWithProtectedRg | DeniedFileRules::GlobsWithNativeFallback
    );
    let temp = if use_controlled_rg_path {
        // Bazel can mount the default temp directory noexec.
        tempfile::tempdir_in(std::env::current_dir().expect("current directory"))
            .or_else(|_| tempfile::tempdir())
    } else {
        tempfile::tempdir()
    }
    .expect("tempdir");
    let workspace = AbsolutePathBuf::try_from(temp.path()).expect("absolute workspace");
    std::fs::create_dir(workspace.join("nested")).expect("create nested directory");
    std::fs::write(workspace.join("AGENTS.md"), "project instructions\n")
        .expect("write allowed instructions");
    let denied_files = [
        (workspace.join("one.key"), "dummy first key"),
        (workspace.join("nested/two.key"), "dummy second key"),
        (workspace.join(".env.local"), "dummy environment"),
    ];
    for (path, contents) in &denied_files {
        std::fs::write(path, contents).expect("write denied dummy file");
    }

    let sandbox_helper = codex_linux_sandbox_exe();
    let helper_dir = AbsolutePathBuf::try_from(sandbox_helper.parent().expect("helper parent"))
        .expect("absolute helper directory");
    let mut entries = vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Minimal,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(helper_dir.into(), FileSystemAccessMode::Read),
        FileSystemSandboxEntry::new(workspace.clone().into(), FileSystemAccessMode::Write),
    ];
    let mut env = create_env_from_core_vars();
    // User config must not suppress the files used to construct deny masks.
    let rg_config = workspace.join("ripgrep.conf");
    std::fs::write(rg_config.as_path(), "--quiet\n").expect("write ripgrep config");
    env.insert(
        "RIPGREP_CONFIG_PATH".to_string(),
        rg_config.to_str().expect("UTF-8 config path").to_string(),
    );
    let rg_fixture = if use_controlled_rg_path {
        let fixture = tempfile::tempdir_in(temp.path().parent().expect("workspace parent"))
            .expect("rg fixture");
        write_executable(
            workspace.join("rg").as_path(),
            "#!/bin/sh\n: > \"$CODEX_TEST_REJECTED_RG_MARKER\"\nexit 0\n",
        )
        .expect("write workspace rg fixture");
        if matches!(rules, DeniedFileRules::GlobsWithProtectedRg) {
            write_executable(
                &fixture.path().join("rg"),
                r#"#!/bin/sh
: > "$CODEX_TEST_PROTECTED_RG_MARKER"
printf '%s\000' "$CODEX_TEST_DENIED_PATH_0" "$CODEX_TEST_DENIED_PATH_1" "$CODEX_TEST_DENIED_PATH_2"
"#,
            )
            .expect("write protected rg fixture");
        }
        // Keep bubblewrap available without adding an unrelated rg to the test PATH.
        if let Some(bwrap) = codex_sandboxing::find_system_bwrap_in_path(
            &PermissionProfile::read_only().file_system_sandbox_policy(),
            &std::env::current_dir().expect("current directory"),
        ) {
            std::os::unix::fs::symlink(bwrap, fixture.path().join("bwrap"))
                .expect("link system bubblewrap");
        }
        env.insert(
            "PATH".to_string(),
            std::env::join_paths([workspace.as_path(), fixture.path()])
                .expect("join PATH")
                .into_string()
                .expect("UTF-8 PATH"),
        );
        for (name, path) in [
            (
                "CODEX_TEST_REJECTED_RG_MARKER",
                fixture.path().join("rejected-rg-ran"),
            ),
            (
                "CODEX_TEST_PROTECTED_RG_MARKER",
                fixture.path().join("protected-rg-ran"),
            ),
            ("CODEX_TEST_DENIED_PATH_0", denied_files[0].0.to_path_buf()),
            ("CODEX_TEST_DENIED_PATH_1", denied_files[1].0.to_path_buf()),
            ("CODEX_TEST_DENIED_PATH_2", denied_files[2].0.to_path_buf()),
        ] {
            env.insert(
                name.to_string(),
                path.to_str().expect("UTF-8 fixture path").to_string(),
            );
        }
        Some(fixture)
    } else {
        None
    };
    match rules {
        DeniedFileRules::ExactPaths => {
            entries.extend(denied_files.iter().map(|(path, _)| {
                FileSystemSandboxEntry::new(path.clone().into(), FileSystemAccessMode::Deny)
            }));
        }
        DeniedFileRules::Globs
        | DeniedFileRules::GlobsWithProtectedRg
        | DeniedFileRules::GlobsWithNativeFallback => {
            for suffix in ["**/*.key", "**/.env.local"] {
                entries.push(FileSystemSandboxEntry::new(
                    FileSystemPath::GlobPattern {
                        pattern: format!("{}/{suffix}", workspace.display()),
                    },
                    FileSystemAccessMode::Deny,
                ));
            }
        }
    }
    let permission_profile = PermissionProfile::from_runtime_permissions(
        &FileSystemSandboxPolicy::restricted(entries),
        NetworkSandboxPolicy::Enabled,
    );
    if matches!(rules, DeniedFileRules::Globs)
        && codex_sandboxing::find_pre_sandbox_executable_in_path(
            "rg",
            &permission_profile.file_system_sandbox_policy(),
            workspace.as_path(),
        )
        .is_none()
    {
        eprintln!("skipping ripgrep config test: no protected rg is available");
        return;
    }
    let output = run_cmd_result_with_permission_profile_for_cwd(
        &[
            "/bin/sh",
            "-c",
            r#"set -eu
/bin/cat AGENTS.md
for blocked in one.key nested/two.key .env.local; do
    if (: < "$blocked") 2>/dev/null; then
        printf 'read unexpectedly allowed: %s\n' "$blocked" >&2
        exit 10
    fi
    if (printf changed > "$blocked") 2>/dev/null; then
        printf 'write unexpectedly allowed: %s\n' "$blocked" >&2
        exit 11
    fi
done
printf 'allowed\n' > allowed.txt
/bin/cat allowed.txt
"#,
        ],
        workspace.clone(),
        permission_profile,
        env,
        LONG_TIMEOUT_MS,
        /*use_legacy_landlock*/ false,
    )
    .await
    .expect("sandbox should start with multiple denied files");

    assert_eq!(
        (output.exit_code, output.stdout.text, output.stderr.text),
        (
            0,
            "project instructions\nallowed\n".to_string(),
            String::new()
        )
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("allowed.txt")).expect("read allowed file"),
        "allowed\n"
    );
    for (path, contents) in &denied_files {
        assert_eq!(
            std::fs::read_to_string(path).expect("read host dummy file"),
            *contents
        );
    }
    if let Some(fixture) = rg_fixture {
        assert!(
            !fixture.path().join("rejected-rg-ran").exists(),
            "writable rg ran before confinement"
        );
        assert_eq!(
            fixture.path().join("protected-rg-ran").exists(),
            matches!(rules, DeniedFileRules::GlobsWithProtectedRg),
        );
    }
}
