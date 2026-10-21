#![cfg(target_os = "windows")]

use super::WindowsSandboxSessionRequest;
use super::spawn_windows_sandbox_session_elevated_for_permission_profile;
use super::spawn_windows_sandbox_session_for_level;
use super::spawn_windows_sandbox_session_legacy;
use crate::WindowsSandboxCancellationToken;
use crate::ipc_framed::Message;
use crate::ipc_framed::decode_bytes;
use crate::ipc_framed::read_frame;
use crate::run_windows_sandbox_capture;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_pty::ProcessDriver;
use codex_utils_pty::ProcessSignal;
use codex_windows_sandbox_test_support::WindowsSandboxAccountTestGuard;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;
use tokio::runtime::Builder;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::WAIT_FAILED;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::Security::ACCESS_ALLOWED_ACE;
use windows_sys::Win32::Security::ACCESS_DENIED_ACE;
use windows_sys::Win32::Security::ACE_HEADER;
use windows_sys::Win32::Security::ACL;
use windows_sys::Win32::Security::ACL_SIZE_INFORMATION;
use windows_sys::Win32::Security::AclSizeInformation;
use windows_sys::Win32::Security::EqualSid;
use windows_sys::Win32::Security::GetAce;
use windows_sys::Win32::Security::GetAclInformation;
use windows_sys::Win32::Security::GetLengthSid;
use windows_sys::Win32::Storage::FileSystem::DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

static TEST_HOME_COUNTER: AtomicU64 = AtomicU64::new(0);
static LEGACY_PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

const ASSERT_NO_CONSOLE: &str = r#"Add-Type -ErrorAction Stop 'using System; using System.Runtime.InteropServices; public class ConsoleProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }'; if ([ConsoleProbe]::GetConsoleWindow() -ne [IntPtr]::Zero) { throw 'piped sandbox process unexpectedly has a console' };"#;

fn legacy_process_test_guard() -> MutexGuard<'static, ()> {
    LEGACY_PROCESS_TEST_LOCK
        .lock()
        .expect("legacy Windows sandbox process test lock poisoned")
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn pwsh_path() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles")?;
    let path = PathBuf::from(program_files).join("PowerShell\\7\\pwsh.exe");
    path.is_file().then_some(path)
}

fn sandbox_cwd() -> PathBuf {
    if let Ok(workspace_root) = std::env::var("INSTA_WORKSPACE_ROOT") {
        return PathBuf::from(workspace_root);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn sandbox_home(name: &str) -> TempDir {
    let id = TEST_HOME_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("codex-windows-sandbox-{name}-{id}"));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create sandbox home");
    tempfile::TempDir::new_in(&path).expect("create sandbox home tempdir")
}

fn sandbox_log(codex_home: &Path) -> String {
    let log_path = crate::current_log_file_path(&codex_home.join(".sandbox"));
    fs::read_to_string(&log_path)
        .unwrap_or_else(|err| format!("failed to read {}: {err}", log_path.display()))
}

const ALLOWED_ACE_TYPE: u8 = 0;
const DENIED_ACE_TYPE: u8 = 1;
const CONTAINER_INHERIT_ACE_FLAG: u8 = 0x02;
const INHERITED_ACE_FLAG: u8 = 0x10;
const INHERIT_ONLY_ACE_FLAG: u8 = 0x08;

/// ACL evidence for the write/delete leg. The verdicts answer the one question the
/// leg is about — can this session's capability SIDs reach a delete right on this
/// path — and each probe path then decodes every ACE it holds, inherited entries
/// included, read through the same handle-based fetch the production ACL code uses,
/// so every line is an entry the sandbox child's access check actually saw. The
/// whole text is printed on every run, green included. Derivation failures are
/// rendered into the report instead of panicking: the evidence must never become
/// the reason a leg goes red.
fn write_delete_acl_evidence(
    permission_profile: &PermissionProfile,
    workspace: &Path,
    codex_home: &Path,
    env_map: &HashMap<String, String>,
    subjects: &[PathBuf],
    probe_paths: &[PathBuf],
) -> String {
    let mut root_sids: Vec<crate::spawn_prep::RootCapabilitySid> = Vec::new();
    let mut dump = String::new();
    match crate::resolved_permissions::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
        permission_profile,
        &workspace_roots_for(workspace),
    ) {
        Ok(permissions) => {
            let capability_roots = crate::spawn_prep::legacy_session_capability_roots(
                &permissions,
                workspace,
                env_map,
                codex_home,
            );
            match crate::spawn_prep::root_capability_sids(codex_home, workspace, capability_roots)
            {
                Ok(sids) => root_sids = sids,
                Err(err) => {
                    dump.push_str(&format!("root capability SIDs failed: {err}\n"));
                }
            }
        }
        Err(err) => dump.push_str(&format!("permissions failed to resolve: {err}\n")),
    }
    let cap_ptrs: Vec<*mut c_void> = root_sids.iter().map(|root| root.sid.as_ptr()).collect();
    let cap_sid_strings = root_sids
        .iter()
        .map(|root| root.sid_str.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut verdicts = format!("writable-root capability SIDs: [{cap_sid_strings}]\n");
    for subject in subjects {
        // Deleting a file needs DELETE on the file or FILE_DELETE_CHILD on its parent,
        // so both halves are reported.
        let on_file = delete_right_verdict(subject.as_path(), &cap_ptrs);
        let on_parent = match subject.parent() {
            Some(parent) => delete_right_verdict(parent, &cap_ptrs),
            None => "no parent".to_string(),
        };
        verdicts.push_str(&format!(
            "{}: session delete rights here=[{on_file}] in parent=[{on_parent}]\n",
            subject.display()
        ));
    }
    dump.push_str(&format!(
        "writable-root capability SIDs: [{cap_sid_strings}]\n{verdicts}"
    ));
    for path in probe_paths {
        dump.push_str(&delete_acl_lines(path, &cap_ptrs));
        dump.push('\n');
    }
    dump
}

fn delete_right_verdict(path: &Path, cap_ptrs: &[*mut c_void]) -> String {
    if cap_ptrs.is_empty() {
        return "capability SIDs unavailable".to_string();
    }
    if !path.exists() {
        return "absent".to_string();
    }
    let (allowed, denied) = match unsafe { session_delete_masks(path, cap_ptrs) } {
        Ok(masks) => masks,
        Err(err) => return format!("unreadable: {err}"),
    };
    let wanted = DELETE | FILE_DELETE_CHILD;
    let granted = allowed & wanted;
    let blocked = denied & wanted;
    let effective = granted & !blocked;
    if effective != 0 {
        mask_names(effective)
    } else if granted != 0 {
        // The writable-root grant reaches the path, but a deny ACE for the same
        // principal takes the right back. Reporting only the grant here would read
        // as "the sandbox can delete this", which the session proves false.
        format!(
            "none({} granted, {} denied)",
            mask_names(granted),
            mask_names(blocked)
        )
    } else {
        "none".to_string()
    }
}

fn mask_names(mask: u32) -> String {
    [(DELETE, "DELETE"), (FILE_DELETE_CHILD, "FILE_DELETE_CHILD")]
        .into_iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join("+")
}

/// Delete-right bits this session's capability SIDs hold on `path`, split into the
/// allow and deny halves. A deny ACE for the same principal takes the right back
/// wherever it sits in the DACL, and an `INHERIT_ONLY` ACE does not apply to the
/// object that carries it, so both are accounted for rather than left to whoever
/// reads the receipt. Generic bits are reported unexpanded: every grant this
/// sandbox writes is an explicit mask.
unsafe fn session_delete_masks(
    path: &Path,
    cap_ptrs: &[*mut c_void],
) -> Result<(u32, u32), String> {
    let (p_dacl, p_sd) = crate::acl::fetch_dacl_handle(path).map_err(|err| err.to_string())?;
    if p_dacl.is_null() {
        let _ = LocalFree(p_sd);
        return Ok((FILE_DELETE_CHILD | DELETE, 0));
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        p_dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        let err = std::io::Error::last_os_error().to_string();
        let _ = LocalFree(p_sd);
        return Err(err);
    }
    let (mut allowed, mut denied) = (0u32, 0u32);
    for index in 0..(info.AceCount as usize) {
        let mut p_ace: *mut c_void = std::ptr::null_mut();
        if GetAce(p_dacl as *const ACL, index as u32, &mut p_ace) == 0 {
            continue;
        }
        let header = &*(p_ace as *const ACE_HEADER);
        if header.AceFlags & INHERIT_ONLY_ACE_FLAG != 0 {
            continue;
        }
        let sid = (p_ace as usize + std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
            as *mut c_void;
        if !cap_ptrs.iter().any(|cap| EqualSid(sid, *cap) != 0) {
            continue;
        }
        match header.AceType {
            ALLOWED_ACE_TYPE => allowed |= (&*(p_ace as *const ACCESS_ALLOWED_ACE)).Mask,
            DENIED_ACE_TYPE => denied |= (&*(p_ace as *const ACCESS_DENIED_ACE)).Mask,
            _ => {}
        }
    }
    let _ = LocalFree(p_sd);
    Ok((allowed, denied))
}

fn delete_acl_lines(path: &Path, cap_ptrs: &[*mut c_void]) -> String {
    if !path.exists() {
        return format!("{}: absent", path.display());
    }
    let (p_dacl, p_sd) = match unsafe { crate::acl::fetch_dacl_handle(path) } {
        Ok(fetched) => fetched,
        Err(err) => return format!("{}: DACL unreadable: {err}", path.display()),
    };
    let lines = unsafe { dacl_aces_text(path, p_dacl, cap_ptrs) };
    unsafe {
        let _ = LocalFree(p_sd);
    }
    lines
}

unsafe fn dacl_aces_text(path: &Path, p_dacl: *mut ACL, cap_ptrs: &[*mut c_void]) -> String {
    if p_dacl.is_null() {
        return format!(
            "{}: NULL DACL, so every SID including the sandbox capability holds FILE_ALL_ACCESS",
            path.display()
        );
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        p_dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return format!(
            "{}: GetAclInformation failed: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    let mut lines = vec![format!(
        "{}: {} ACE(s)",
        path.display(),
        info.AceCount as usize
    )];
    for index in 0..(info.AceCount as usize) {
        let mut p_ace: *mut c_void = std::ptr::null_mut();
        if GetAce(p_dacl as *const ACL, index as u32, &mut p_ace) == 0 {
            lines.push(format!("  ace[{index}]: GetAce failed"));
            continue;
        }
        let header = &*(p_ace as *const ACE_HEADER);
        let (kind, mask) = match header.AceType {
            ALLOWED_ACE_TYPE => ("allow", (&*(p_ace as *const ACCESS_ALLOWED_ACE)).Mask),
            DENIED_ACE_TYPE => ("deny", (&*(p_ace as *const ACCESS_DENIED_ACE)).Mask),
            other => {
                lines.push(format!("  ace[{index}]: unhandled ACE type {other}"));
                continue;
            }
        };
        let sid = (p_ace as usize + std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
            as *mut c_void;
        let sid_len = GetLengthSid(sid);
        let sid_text = if sid_len == 0 {
            "unreadable".to_string()
        } else {
            let bytes = std::slice::from_raw_parts(sid as *const u8, sid_len as usize);
            crate::winutil::string_from_sid_bytes(bytes).unwrap_or_else(|err| format!("<{err}>"))
        };
        let is_session_capability = cap_ptrs.iter().any(|cap| EqualSid(sid, *cap) != 0);
        let delete_rights = mask_names(mask);
        lines.push(format!(
            "  ace[{index}] {kind} mask=0x{mask:08x} flags=0x{:02x}{}{} sid={sid_text}{} \
             delete-rights=[{delete_rights}]",
            header.AceFlags,
            if header.AceFlags & CONTAINER_INHERIT_ACE_FLAG != 0 {
                " inherits-to-children"
            } else {
                ""
            },
            if header.AceFlags & INHERITED_ACE_FLAG != 0 {
                " INHERITED-FROM-ANCESTOR"
            } else {
                ""
            },
            if is_session_capability {
                " <-- THIS SESSION"
            } else {
                ""
            },
        ));
    }
    lines.join("\n")
}

fn ancestors_up_to_volume_root(path: &Path) -> Vec<PathBuf> {
    let mut ancestors = Vec::new();
    let mut cursor = path.parent();
    while let Some(parent) = cursor {
        ancestors.push(parent.to_path_buf());
        cursor = parent.parent();
    }
    ancestors
}

fn workspace_roots_for(root: &Path) -> Vec<AbsolutePathBuf> {
    vec![AbsolutePathBuf::from_absolute_path(root).expect("absolute workspace root")]
}

fn powershell_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn start_powershell_child(
    pwsh: &Path,
    stdio_dir: &Path,
    child_command: &str,
    parent_tail: &str,
) -> String {
    let encoded = BASE64.encode(
        child_command
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!(
        "Start-Process -WindowStyle Hidden -FilePath '{}' -ArgumentList '-NoProfile','-EncodedCommand','{encoded}' -RedirectStandardOutput '{}' -RedirectStandardError '{}'; {parent_tail}",
        powershell_literal(pwsh),
        powershell_literal(&stdio_dir.join("descendant.stdout")),
        powershell_literal(&stdio_dir.join("descendant.stderr")),
    )
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    path.exists()
}

fn open_process_for_wait(pid: u32) -> std::io::Result<OwnedHandle> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}

fn wait_for_process_exit(process: &OwnedHandle, timeout: Duration) -> std::io::Result<()> {
    let timeout_ms = u32::try_from(timeout.as_millis())
        .map_err(|_| std::io::Error::other("process wait timeout exceeds u32"))?;
    let result = unsafe { WaitForSingleObject(process.as_raw_handle() as _, timeout_ms) };
    match result {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out waiting for process to exit",
        )),
        WAIT_FAILED => Err(std::io::Error::last_os_error()),
        result => Err(std::io::Error::other(format!(
            "unexpected process wait result: {result}"
        ))),
    }
}

fn wait_for_frame_count(frames_path: &Path, expected_frames: usize) -> Vec<Message> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut reader = OpenOptions::new()
            .read(true)
            .open(frames_path)
            .expect("open frame file for read");
        reader
            .seek(SeekFrom::Start(0))
            .expect("seek to start of frame file");

        let mut frames = Vec::new();
        loop {
            match read_frame(&mut reader) {
                Ok(Some(frame)) => frames.push(frame.message),
                Ok(None) => break,
                Err(_) => break,
            }
        }

        if frames.len() >= expected_frames {
            return frames;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected_frames} frames, saw {}",
            frames.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn collect_stdout_and_exit(
    spawned: codex_utils_pty::SpawnedProcess,
    codex_home: &Path,
    timeout_duration: Duration,
) -> (Vec<u8>, i32) {
    let codex_utils_pty::SpawnedProcess {
        session: _session,
        mut stdout_rx,
        stderr_rx: _stderr_rx,
        exit_rx,
    } = spawned;
    let stdout_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        while let Some(chunk) = stdout_rx.recv().await {
            stdout.extend(chunk);
        }
        stdout
    });
    let exit_code = timeout(timeout_duration, exit_rx)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for exit\n{}", sandbox_log(codex_home)))
        .unwrap_or(-1);
    let stdout = timeout(timeout_duration, stdout_task)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "timed out waiting for stdout task\n{}",
                sandbox_log(codex_home)
            )
        })
        .expect("stdout task join");
    (stdout, exit_code)
}

#[test]
fn restricted_token_rejects_managed_network_before_spawn() {
    current_thread_runtime().block_on(async {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("restricted-token-managed-network");
        let permission_profile = PermissionProfile::workspace_write();
        let error = spawn_windows_sandbox_session_for_level(WindowsSandboxSessionRequest {
            permission_profile: &permission_profile,
            workspace_roots: &[],
            codex_home: codex_home.path(),
            command: Vec::new(),
            cwd: cwd.as_path(),
            env_map: HashMap::new(),
            windows_sandbox_level: WindowsSandboxLevel::RestrictedToken,
            proxy_enforced: true,
            network_proxy_restricting_sid: None,
            proxy_settings_mode: crate::WindowsSandboxProxySettingsMode::Preserve,
            timeout_ms: None,
            read_roots_override: None,
            read_roots_include_platform_defaults: false,
            write_roots_override: None,
            deny_read_paths_override: &[],
            deny_write_paths_override: &[],
            tty: false,
            stdin_open: false,
        })
        .await
        .expect_err("managed networking must fail before spawning an unelevated sandbox");

        assert_eq!(
            error.to_string(),
            "managed networking requires the elevated Windows sandbox backend"
        );
    });
}

#[test]
fn legacy_non_tty_cmd_emits_output() {
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("legacy-non-tty-cmd");
        println!("cmd codex_home={}", codex_home.path().display());
        let permission_profile = PermissionProfile::workspace_write();
        let spawned = spawn_windows_sandbox_session_legacy(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                "C:\\Windows\\System32\\cmd.exe".to_string(),
                "/c".to_string(),
                "echo LEGACY-NONTTY-CMD".to_string(),
            ],
            cwd.as_path(),
            HashMap::new(),
            Some(5_000),
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
        )
        .await
        .expect("spawn legacy non-tty cmd session");
        println!("cmd spawn returned");
        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(10)).await;
        println!("cmd collect returned exit_code={exit_code}");
        let stdout = String::from_utf8_lossy(&stdout);
        assert_eq!(exit_code, 0, "stdout={stdout:?}");
        assert!(stdout.contains("LEGACY-NONTTY-CMD"), "stdout={stdout:?}");
    });
}

#[test]
fn elevated_non_tty_cmd_forwards_env_output_and_exit() {
    let _account_guard =
        WindowsSandboxAccountTestGuard::acquire().expect("lock Windows sandbox test accounts");
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("elevated-non-tty-cmd");
        let permission_profile = PermissionProfile::workspace_write();
        let env_map = HashMap::from([(
            "CODEX_ELEVATED_TEST".to_string(),
            "ELEVATED-ENV-OK".to_string(),
        )]);
        let spawned = spawn_windows_sandbox_session_elevated_for_permission_profile(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                "C:\\Windows\\System32\\cmd.exe".to_string(),
                "/d".to_string(),
                "/c".to_string(),
                "echo %CODEX_ELEVATED_TEST% & exit /b 23".to_string(),
            ],
            cwd.as_path(),
            env_map,
            /*proxy_enforced*/ false,
            /*network_proxy_restricting_sid*/ None,
            Some(5_000),
            /*read_roots_override*/ None,
            /*read_roots_include_platform_defaults*/ true,
            /*write_roots_override*/ None,
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
        )
        .await
        .unwrap_or_else(|err| {
            panic!(
                "spawn elevated non-tty cmd session: {err:#}\nsandbox log:\n{}",
                sandbox_log(codex_home.path())
            )
        });
        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(10)).await;
        let stdout = String::from_utf8_lossy(&stdout);
        assert_eq!(exit_code, 23, "stdout={stdout:?}");
        assert!(stdout.contains("ELEVATED-ENV-OK"), "stdout={stdout:?}");
    });
}

#[test]
#[ignore = "requires this test binary in an installed test MSIX, launched with package identity, CODEX_WINDOWS_REGISTERED_CORE=1, and CODEX_HOME provisioned by that package's service in a disposable Windows VM"]
fn registered_non_tty_cmd_forwards_env_output_and_exit() {
    assert!(
        crate::registered_core_requested(),
        "registered Core opt-in is required"
    );
    let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").expect("fixture CODEX_HOME"));
    assert!(
        crate::app_package::registered_setup_is_ready(&codex_home)
            .expect("validate the installed package and service receipt"),
        "the test process must have package identity and completed service setup",
    );
    let cwd = tempfile::tempdir().expect("isolated command workspace");
    current_thread_runtime().block_on(async {
        let mut env_map: HashMap<String, String> = std::env::vars().collect();
        env_map.insert("CODEX_REGISTERED_TEST".into(), "REGISTERED-ENV-OK".into());
        let spawned = spawn_windows_sandbox_session_elevated_for_permission_profile(
            &PermissionProfile::workspace_write(),
            workspace_roots_for(cwd.path()).as_slice(),
            &codex_home,
            vec![
                "C:\\Windows\\System32\\cmd.exe".into(),
                "/d".into(),
                "/c".into(),
                "echo %CODEX_REGISTERED_TEST%& exit /b 23".into(),
            ],
            cwd.path(),
            env_map,
            /*proxy_enforced*/ false,
            /*network_proxy_restricting_sid*/ None,
            Some(5_000),
            /*read_roots_override*/ None,
            /*read_roots_include_platform_defaults*/ true,
            /*write_roots_override*/ None,
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
        )
        .await
        .expect("launch through the service-recorded alias and authenticated pipes");
        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, &codex_home, Duration::from_secs(10)).await;
        assert_eq!(
            (
                String::from_utf8(stdout).expect("command output"),
                exit_code
            ),
            ("REGISTERED-ENV-OK\r\n".to_owned(), 23),
        );
    });
}

#[test]
fn legacy_non_tty_cmd_rejects_deny_read_overrides() {
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("legacy-non-tty-deny-read");
        let secret_path =
            AbsolutePathBuf::from_absolute_path(cwd.join("legacy-non-tty-deny-read-secret.env"))
                .expect("absolute deny-read fixture path");
        let permission_profile = PermissionProfile::workspace_write();
        let err = spawn_windows_sandbox_session_legacy(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                "C:\\Windows\\System32\\cmd.exe".to_string(),
                "/c".to_string(),
                "echo deny-read".to_string(),
            ],
            cwd.as_path(),
            HashMap::new(),
            Some(5_000),
            std::slice::from_ref(&secret_path),
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
        )
        .await
        .expect_err("legacy deny-read should require the elevated backend");
        assert!(
            err.to_string()
                .contains("deny-read overrides require the elevated Windows sandbox backend"),
            "unexpected error: {err:#}"
        );
    });
}

#[test]
fn legacy_non_tty_powershell_interrupt_terminates_process() {
    let Some(pwsh) = pwsh_path() else {
        return;
    };
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("legacy-non-tty-pwsh");
        println!("pwsh codex_home={}", codex_home.path().display());
        let permission_profile = PermissionProfile::workspace_write();
        let spawned = spawn_windows_sandbox_session_legacy(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                pwsh.display().to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("{ASSERT_NO_CONSOLE} Write-Output LEGACY-NONTTY-DIRECT; [System.Threading.ManualResetEvent]::new($false).WaitOne()"),
            ],
            cwd.as_path(),
            HashMap::new(),
            /*timeout_ms*/ None,
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
            )
        .await
        .expect("spawn legacy non-tty powershell session");
        println!("pwsh spawn returned");
        let codex_utils_pty::SpawnedProcess {
            session,
            mut stdout_rx,
            stderr_rx: _stderr_rx,
            exit_rx,
        } = spawned;
        timeout(Duration::from_secs(5), async {
            let mut stdout = Vec::new();
            while let Some(chunk) = stdout_rx.recv().await {
                stdout.extend(chunk);
                if String::from_utf8_lossy(&stdout).contains("LEGACY-NONTTY-DIRECT") {
                    return;
                }
            }
            panic!(
                "PowerShell exited before emitting its readiness marker\n{}",
                sandbox_log(codex_home.path())
            );
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "timed out waiting for PowerShell readiness marker\n{}",
                sandbox_log(codex_home.path())
            )
        });

        session
            .signal(ProcessSignal::Interrupt)
            .expect("interrupt should terminate the restricted-token process job");
        let exit_code = timeout(Duration::from_secs(5), exit_rx)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for exit\n{}", sandbox_log(codex_home.path())))
            .expect("interrupted process should report an exit code");
        assert_eq!(exit_code, 1);
    });
}

#[test]
fn finish_driver_spawn_keeps_stdin_open_when_requested() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(1);
        let (_stdout_tx, stdout_rx) = broadcast::channel::<Vec<u8>>(1);
        let (exit_tx, exit_rx) = oneshot::channel::<i32>();
        drop(exit_tx);

        let spawned = super::finish_driver_spawn(
            ProcessDriver {
                writer_tx,
                stdout_rx,
                stderr_rx: None,
                exit_rx,
                terminator: None,
                writer_handle: None,
                resizer: None,
                tty: false,
            },
            /*stdin_open*/ true,
        );

        spawned
            .session
            .writer_sender()
            .send(b"open".to_vec())
            .await
            .expect("stdin should stay open");
        assert_eq!(writer_rx.recv().await, Some(b"open".to_vec()));
    });
}

#[test]
fn finish_driver_spawn_closes_stdin_when_not_requested() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let (writer_tx, _writer_rx) = mpsc::channel::<Vec<u8>>(1);
        let (_stdout_tx, stdout_rx) = broadcast::channel::<Vec<u8>>(1);
        let (exit_tx, exit_rx) = oneshot::channel::<i32>();
        drop(exit_tx);

        let spawned = super::finish_driver_spawn(
            ProcessDriver {
                writer_tx,
                stdout_rx,
                stderr_rx: None,
                exit_rx,
                terminator: None,
                writer_handle: None,
                resizer: None,
                tty: false,
            },
            /*stdin_open*/ false,
        );

        assert!(
            spawned
                .session
                .writer_sender()
                .send(b"closed".to_vec())
                .await
                .is_err(),
            "stdin should be closed when streaming input is disabled"
        );
    });
}

#[test]
fn runner_stdin_writer_sends_close_stdin_after_input_eof() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let tempdir = TempDir::new().expect("create tempdir");
        let frames_path = tempdir.path().join("runner-stdin-frames.bin");
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&frames_path)
            .expect("create frame file");
        let outbound_tx = super::start_runner_pipe_writer(file);
        let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(1);
        let writer_handle = super::start_runner_stdin_writer(
            writer_rx,
            outbound_tx,
            /*normalize_newlines*/ false,
            /*stdin_open*/ true,
        );

        writer_tx
            .send(b"hello".to_vec())
            .await
            .expect("send stdin bytes");
        drop(writer_tx);
        writer_handle.await.expect("join stdin writer");

        let frames = wait_for_frame_count(&frames_path, 2);

        match &frames[0] {
            Message::Stdin { payload } => {
                let bytes = decode_bytes(&payload.data_b64).expect("decode stdin payload");
                assert_eq!(bytes, b"hello".to_vec());
            }
            other => panic!("expected stdin frame, got {other:?}"),
        }

        match &frames[1] {
            Message::CloseStdin { .. } => {}
            other => panic!("expected close-stdin frame, got {other:?}"),
        }
    });
}

#[test]
fn runner_resizer_sends_resize_frame() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let tempdir = TempDir::new().expect("create tempdir");
        let frames_path = tempdir.path().join("runner-resize-frames.bin");
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&frames_path)
            .expect("create frame file");
        let outbound_tx = super::start_runner_pipe_writer(file);
        let mut resizer = super::make_runner_resizer(outbound_tx);

        resizer(codex_utils_pty::TerminalSize {
            rows: 45,
            cols: 132,
        })
        .expect("send resize frame");

        let frames = wait_for_frame_count(&frames_path, 1);
        match &frames[0] {
            Message::Resize { payload } => {
                assert_eq!(payload.rows, 45);
                assert_eq!(payload.cols, 132);
            }
            other => panic!("expected resize frame, got {other:?}"),
        }
    });
}

#[test]
fn legacy_capture_emits_output_and_preserves_descendant_after_normal_exit() {
    let Some(pwsh) = pwsh_path() else {
        return;
    };
    let _guard = legacy_process_test_guard();
    let cwd = sandbox_cwd();
    let codex_home = sandbox_home("legacy-capture-pwsh");
    println!("capture pwsh codex_home={}", codex_home.path().display());
    let ready_marker = codex_home.path().join("descendant-started");
    let release_marker = codex_home.path().join("release-descendant");
    let survival_marker = codex_home.path().join("descendant-survived");
    let descendant_command = format!(
        "$deadline=(Get-Date).AddSeconds(30); Set-Content -LiteralPath '{}' -Value $PID; while (-not (Test-Path -LiteralPath '{}')) {{ if ((Get-Date) -ge $deadline) {{ exit 3 }}; Start-Sleep -Milliseconds 25 }}; Set-Content -LiteralPath '{}' -Value survived",
        powershell_literal(&ready_marker),
        powershell_literal(&release_marker),
        powershell_literal(&survival_marker),
    );
    let parent_tail = format!(
        "while (-not (Test-Path -LiteralPath '{}')) {{ Start-Sleep -Milliseconds 25 }}",
        powershell_literal(&ready_marker),
    );
    let parent_command = format!(
        "{ASSERT_NO_CONSOLE} Write-Output LEGACY-CAPTURE-DIRECT; {}",
        start_powershell_child(&pwsh, codex_home.path(), &descendant_command, &parent_tail,),
    );
    let permission_profile = PermissionProfile::workspace_write();
    let result = run_windows_sandbox_capture(
        &permission_profile,
        workspace_roots_for(cwd.as_path()).as_slice(),
        codex_home.path(),
        vec![
            pwsh.display().to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            parent_command,
        ],
        cwd.as_path(),
        HashMap::new(),
        // The parent command does not exit until the sandboxed grandchild writes its pid
        // marker, so this ceiling is what that marker wait gets. Hosted runners needed
        // more than the old 10s: the capture returned as a timeout and the marker read
        // then panicked with "read descendant pid: NotFound" at 12.515s / 11.472s /
        // 11.324s (runs 39371971338, 39347345560, 38824922110). See the measurement in
        // legacy_capture_cancellation_terminates_descendants_without_timeout.
        Some(60_000),
        /*cancellation*/ None,
    )
    .expect("run legacy capture powershell");
    let descendant_pid = fs::read_to_string(&ready_marker)
        .expect("read descendant pid")
        .trim()
        .parse()
        .expect("parse descendant pid");
    let descendant_process = open_process_for_wait(descendant_pid);
    fs::write(&release_marker, "release").expect("release descendant after root exit");
    let descendant_process = descendant_process.expect("open descendant after normal capture exit");

    println!("capture pwsh exit_code={}", result.exit_code);
    println!("capture pwsh timed_out={}", result.timed_out);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    println!("capture pwsh stderr={stderr:?}");
    assert_eq!(result.exit_code, 0, "stdout={stdout:?} stderr={stderr:?}");
    assert!(
        stdout.contains("LEGACY-CAPTURE-DIRECT"),
        "stdout={stdout:?}"
    );
    assert!(
        wait_for_path(&survival_marker, Duration::from_secs(10)),
        "sandbox descendant did not survive normal capture exit"
    );
    wait_for_process_exit(&descendant_process, Duration::from_secs(10))
        .expect("sandbox descendant did not exit after release");
}

#[test]
fn legacy_workspace_write_delete_is_limited_to_writable_roots() {
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        // Keep writable roots out of USERPROFILE exclusions such as AppData.
        let test_root = TempDir::new_in(sandbox_cwd()).expect("create legacy delete test root");
        let codex_home = sandbox_home("legacy-delete-writable-roots");
        let workspace = test_root.path().join("workspace");
        let temp_root = test_root.path().join("temp");
        let tmp_root = test_root.path().join("tmp");
        let outside_root = test_root.path().join("outside");
        for directory in [&workspace, &temp_root, &tmp_root, &outside_root] {
            fs::create_dir_all(directory).expect("create legacy delete test directory");
        }
        let protected_git_dir = workspace.join(".git");
        fs::create_dir(&protected_git_dir).expect("create protected .git directory");
        // The directory surviving `rmdir` cannot show whether the read-only rule reaches
        // inside it: the writable-root grant is inherited by every descendant, while the
        // `.git` denial is one path. A child of `.git` is the case that tells them apart.
        let protected_git_child = protected_git_dir.join("config");
        fs::write(&protected_git_child, "[core]\n").expect("seed protected .git file");

        let workspace_file = workspace.join("workspace-delete.txt");
        let temp_file = temp_root.join("temp-delete.txt");
        let tmp_file = tmp_root.join("tmp-delete.txt");
        let outside_file = outside_root.join("outside-delete.txt");
        fs::write(&workspace_file, "workspace").expect("seed workspace file");
        fs::write(&temp_file, "temp").expect("seed TEMP file");
        fs::write(&tmp_file, "tmp").expect("seed TMP file");
        fs::write(&outside_file, "outside").expect("seed outside file");

        let script = workspace.join("delete-fixtures.cmd");
        fs::write(
            &script,
            concat!(
                "@echo off\r\n",
                "del /f /q \"%WORKSPACE_DELETE%\"\r\n",
                "del /f /q \"%TEMP_DELETE%\"\r\n",
                "del /f /q \"%TMP_DELETE%\"\r\n",
                "del /f /q \"%OUTSIDE_DELETE%\"\r\n",
                "del /f /q \"%PROTECTED_GIT_CHILD%\"\r\n",
                "rmdir \"%PROTECTED_GIT_DIR%\"\r\n",
                "exit /b 0\r\n",
            ),
        )
        .expect("write delete script");

        let env_map = HashMap::from([
            ("TEMP".to_string(), temp_root.to_string_lossy().into_owned()),
            ("TMP".to_string(), tmp_root.to_string_lossy().into_owned()),
            (
                "WORKSPACE_DELETE".to_string(),
                workspace_file.to_string_lossy().into_owned(),
            ),
            (
                "TEMP_DELETE".to_string(),
                temp_file.to_string_lossy().into_owned(),
            ),
            (
                "TMP_DELETE".to_string(),
                tmp_file.to_string_lossy().into_owned(),
            ),
            (
                "OUTSIDE_DELETE".to_string(),
                outside_file.to_string_lossy().into_owned(),
            ),
            (
                "PROTECTED_GIT_DIR".to_string(),
                protected_git_dir.to_string_lossy().into_owned(),
            ),
            (
                "PROTECTED_GIT_CHILD".to_string(),
                protected_git_child.to_string_lossy().into_owned(),
            ),
        ]);

        let permission_profile = PermissionProfile::workspace_write();
        let acl_subjects: Vec<PathBuf> = vec![
            workspace_file.clone(),
            temp_file.clone(),
            tmp_file.clone(),
            outside_file.clone(),
            protected_git_dir.clone(),
            protected_git_child.clone(),
        ];
        let mut probe_paths: Vec<PathBuf> = vec![
            workspace.clone(),
            temp_root.clone(),
            tmp_root.clone(),
            outside_root.clone(),
            test_root.path().to_path_buf(),
        ];
        probe_paths.extend(acl_subjects.iter().cloned());
        probe_paths.extend(ancestors_up_to_volume_root(test_root.path()));
        let acl_env_map = env_map.clone();
        // Taken before the session starts: a file the sandbox must not delete only
        // has a DACL worth reading while it still exists.
        let acl_dump_before = write_delete_acl_evidence(
            &permission_profile,
            workspace.as_path(),
            codex_home.path(),
            &acl_env_map,
            acl_subjects.as_slice(),
            probe_paths.as_slice(),
        );
        // Printed every run, not only on failure: this leg can go green while the
        // access check still answers "yes, delete allowed" for a protected path,
        // and a green run with no ACE text in the log is the blind spot being closed.
        eprintln!("write/delete ACL evidence before the session:\n{acl_dump_before}");
        let spawned = spawn_windows_sandbox_session_legacy(
            &permission_profile,
            workspace_roots_for(workspace.as_path()).as_slice(),
            codex_home.path(),
            vec![
                "C:\\Windows\\System32\\cmd.exe".to_string(),
                "/d".to_string(),
                "/c".to_string(),
                script.display().to_string(),
            ],
            workspace.as_path(),
            env_map,
            /*timeout_ms*/ Some(5_000),
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
        )
        .await
        .expect("spawn legacy delete session");
        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(/*secs*/ 10))
                .await;
        let stdout = String::from_utf8_lossy(&stdout);
        let acl_dump_after = write_delete_acl_evidence(
            &permission_profile,
            workspace.as_path(),
            codex_home.path(),
            &acl_env_map,
            acl_subjects.as_slice(),
            probe_paths.as_slice(),
        );
        eprintln!("write/delete ACL evidence after the session:\n{acl_dump_after}");

        assert_eq!(
            (
                exit_code,
                workspace_file.exists(),
                temp_file.exists(),
                tmp_file.exists(),
                fs::read_to_string(&outside_file).ok(),
                protected_git_dir.is_dir(),
                protected_git_child.exists(),
            ),
            (
                0,
                false,
                false,
                false,
                Some("outside".to_string()),
                true,
                true
            ),
            "stdout={stdout:?}\n{}\nThe before/after ACL evidence for this session is \
             in the captured stderr above.",
            sandbox_log(codex_home.path()),
        );
    });
}

#[test]
fn legacy_capture_cancellation_terminates_descendants_without_timeout() {
    let Some(pwsh) = pwsh_path() else {
        eprintln!("skipping cancellation regression test: PowerShell 7 is not installed");
        return;
    };
    let _guard = legacy_process_test_guard();
    let cwd = sandbox_cwd();
    let codex_home = sandbox_home("legacy-capture-cancel");
    let descendant_marker = codex_home.path().join("descendant-survived");
    let ready_marker = codex_home.path().join("descendant-started");
    let descendant_command = format!(
        "Set-Content -LiteralPath '{}' -Value $PID; Start-Sleep -Seconds 1; Set-Content -LiteralPath '{}' -Value survived",
        powershell_literal(&ready_marker),
        powershell_literal(&descendant_marker),
    );
    let parent_command = start_powershell_child(
        &pwsh,
        codex_home.path(),
        &descendant_command,
        "Start-Sleep -Seconds 30",
    );
    let descendant_process = Arc::new(Mutex::new(None));
    let descendant_process_for_cancellation = Arc::clone(&descendant_process);
    let cancellation = WindowsSandboxCancellationToken::new(move || {
        let Ok(pid) = fs::read_to_string(&ready_marker).and_then(|pid| {
            pid.trim()
                .parse()
                .map_err(|err| std::io::Error::other(format!("invalid descendant pid: {err}")))
        }) else {
            return false;
        };
        let Ok(process) = open_process_for_wait(pid) else {
            return false;
        };
        *descendant_process_for_cancellation
            .lock()
            .expect("descendant process lock poisoned") = Some(process);
        true
    });

    let started_at = Instant::now();
    let permission_profile = PermissionProfile::workspace_write();
    let result = run_windows_sandbox_capture(
        &permission_profile,
        workspace_roots_for(cwd.as_path()).as_slice(),
        codex_home.path(),
        vec![
            pwsh.display().to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            parent_command,
        ],
        cwd.as_path(),
        HashMap::new(),
        Some(90_000),
        /*cancellation*/ Some(cancellation),
    )
    .expect("run legacy capture powershell with cancellation");

    // The cancellation callback cannot return true until the sandboxed grandchild has
    // written its pid marker, so this measures hosted-runner grandchild start-up, not
    // cancellation latency. Three consecutive runs lost the whole leg to the old 30s
    // capture ceiling: 32.056s / 34.563s / 33.973s (runs 39371971338, 39347345560,
    // 38824922110 - identical text since 2026-10-15), so start-up on `windows-latest`
    // is only bounded below at >30s, against 0.024-0.026s on an idle Windows 11 machine.
    // Reserving every nextest thread for the leg did not change the CI numbers, so this
    // is runner process start-up cost, not contention with sibling tests. The capture
    // ceiling is 90s and this assert is the inner 60s so that a slow leg is decided by
    // These two asserts, not a nextest kill that leaves nothing but TIMEOUT; the worst
    // case here (90s capture + the 10s exit wait below) fits the 4m window this family
    // gets in `.config/nextest.toml`, which is sized by the two back-to-back lifecycles
    // of legacy_tty_job_terminates_and_preserves_descendants. Whether 90s is enough is
    // answered by the run this push triggers, since the CI data gives no upper bound.
    assert!(
        started_at.elapsed() < Duration::from_secs(60),
        "cancellation should end capture before the timeout"
    );
    assert!(
        !result.timed_out,
        "cancellation should not be reported as a timeout"
    );
    assert_ne!(result.exit_code, 0);
    let descendant_process = descendant_process
        .lock()
        .expect("descendant process lock poisoned")
        .take()
        .expect("cancellation did not capture descendant process");
    wait_for_process_exit(&descendant_process, Duration::from_secs(10))
        .expect("sandbox descendant did not exit after cancellation");
    assert!(
        !descendant_marker.exists(),
        "sandbox descendant survived cancellation"
    );
}

#[derive(Clone, Copy, Debug)]
enum LegacyTtyDescendantLifecycle {
    Terminate,
    Preserve,
}

async fn assert_legacy_tty_descendant_lifecycle(
    pwsh: &Path,
    lifecycle: LegacyTtyDescendantLifecycle,
) {
    let cwd = sandbox_cwd();
    let codex_home = sandbox_home(match lifecycle {
        LegacyTtyDescendantLifecycle::Terminate => "legacy-tty-descendant-terminate",
        LegacyTtyDescendantLifecycle::Preserve => "legacy-tty-descendant-preserve",
    });
    let ready_marker = codex_home.path().join("descendant-started");
    let release_marker = codex_home.path().join("release-descendant");
    let survival_marker = codex_home.path().join("descendant-survived");
    let child_tail = match lifecycle {
        LegacyTtyDescendantLifecycle::Terminate => "Start-Sleep -Seconds 30".to_string(),
        LegacyTtyDescendantLifecycle::Preserve => format!(
            "$deadline=(Get-Date).AddSeconds(30); while (-not (Test-Path -LiteralPath '{}')) {{ if ((Get-Date) -ge $deadline) {{ exit 3 }}; Start-Sleep -Milliseconds 25 }}; Set-Content -LiteralPath '{}' -Value survived",
            powershell_literal(&release_marker),
            powershell_literal(&survival_marker),
        ),
    };
    let child_command = format!(
        "Set-Content -LiteralPath '{}' -Value $PID; {child_tail}",
        powershell_literal(&ready_marker),
    );
    let parent_tail = match lifecycle {
        LegacyTtyDescendantLifecycle::Terminate => "Start-Sleep -Seconds 30".to_string(),
        LegacyTtyDescendantLifecycle::Preserve => format!(
            "while (-not (Test-Path -LiteralPath '{}')) {{ Start-Sleep -Milliseconds 25 }}",
            powershell_literal(&ready_marker),
        ),
    };
    let parent_command =
        start_powershell_child(pwsh, codex_home.path(), &child_command, &parent_tail);
    let permission_profile = PermissionProfile::workspace_write();
    let spawned = spawn_windows_sandbox_session_legacy(
        &permission_profile,
        workspace_roots_for(cwd.as_path()).as_slice(),
        codex_home.path(),
        vec![
            pwsh.display().to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            parent_command,
        ],
        cwd.as_path(),
        HashMap::new(),
        // Same runner start-up cost as the capture cases above: the ready-marker wait
        // under this ceiling is what panicked with "Terminate descendant did not start"
        // at 14.163s / 11.051s / 10.861s (runs 39371971338, 39347345560, 38824922110).
        Some(60_000),
        &[],
        &[],
        /*tty*/ true,
        /*stdin_open*/ false,
    )
    .await
    .expect("spawn legacy sandbox ConPTY lifecycle test");
    assert!(
        wait_for_path(&ready_marker, Duration::from_secs(60)),
        "{lifecycle:?} descendant did not start"
    );
    let descendant_pid = fs::read_to_string(&ready_marker)
        .expect("read descendant pid")
        .trim()
        .parse()
        .expect("parse descendant pid");
    let descendant_process = open_process_for_wait(descendant_pid);

    if matches!(lifecycle, LegacyTtyDescendantLifecycle::Terminate) {
        spawned.session.request_terminate();
    }
    let (_, exit_code) =
        collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(15)).await;
    if matches!(lifecycle, LegacyTtyDescendantLifecycle::Preserve) {
        fs::write(&release_marker, "release").expect("release preserved descendant");
    }
    let descendant_process = descendant_process.expect("open sandbox ConPTY descendant");

    match lifecycle {
        LegacyTtyDescendantLifecycle::Terminate => assert_ne!(exit_code, 0),
        LegacyTtyDescendantLifecycle::Preserve => {
            assert_eq!(exit_code, 0);
            assert!(
                wait_for_path(&survival_marker, Duration::from_secs(10)),
                "sandbox ConPTY descendant did not survive normal exit"
            );
        }
    }
    wait_for_process_exit(&descendant_process, Duration::from_secs(10))
        .expect("sandbox ConPTY descendant did not exit");
}

#[test]
fn legacy_tty_job_terminates_and_preserves_descendants() {
    let Some(pwsh) = pwsh_path() else {
        eprintln!("skipping sandbox ConPTY lifecycle test: PowerShell 7 is not installed");
        return;
    };
    let _guard = legacy_process_test_guard();
    current_thread_runtime().block_on(async move {
        assert_legacy_tty_descendant_lifecycle(&pwsh, LegacyTtyDescendantLifecycle::Terminate)
            .await;
        assert_legacy_tty_descendant_lifecycle(&pwsh, LegacyTtyDescendantLifecycle::Preserve).await;
    });
}

#[test]
fn legacy_tty_powershell_emits_output_and_accepts_input() {
    let Some(pwsh) = pwsh_path() else {
        return;
    };
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("legacy-tty-pwsh");
        println!("tty pwsh codex_home={}", codex_home.path().display());
        let permission_profile = PermissionProfile::workspace_write();
        let spawned = spawn_windows_sandbox_session_legacy(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                pwsh.display().to_string(),
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-NoExit".to_string(),
                "-Command".to_string(),
                "$PID; Write-Output ready".to_string(),
            ],
            cwd.as_path(),
            HashMap::new(),
            Some(10_000),
            &[],
            &[],
            /*tty*/ true,
            /*stdin_open*/ true,
        )
        .await
        .expect("spawn legacy tty powershell session");
        println!("tty pwsh spawn returned");

        let writer = spawned.session.writer_sender();
        writer
            .send(b"Write-Output second\n".to_vec())
            .await
            .expect("send second command");
        writer
            .send(b"exit\n".to_vec())
            .await
            .expect("send exit command");
        spawned.session.close_stdin();

        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(15)).await;
        let stdout = String::from_utf8_lossy(&stdout);
        assert_eq!(exit_code, 0, "stdout={stdout:?}");
        assert!(stdout.contains("ready"), "stdout={stdout:?}");
        assert!(stdout.contains("second"), "stdout={stdout:?}");
    });
}
#[test]
#[ignore = "TODO: legacy ConPTY cmd.exe exits with STATUS_DLL_INIT_FAILED in CI"]
fn legacy_tty_cmd_emits_output_and_accepts_input() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("legacy-tty-cmd");
        println!("tty cmd codex_home={}", codex_home.path().display());
        let permission_profile = PermissionProfile::workspace_write();
        let spawned = spawn_windows_sandbox_session_legacy(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                "C:\\Windows\\System32\\cmd.exe".to_string(),
                "/K".to_string(),
                "echo ready".to_string(),
            ],
            cwd.as_path(),
            HashMap::new(),
            Some(10_000),
            &[],
            &[],
            /*tty*/ true,
            /*stdin_open*/ true,
        )
        .await
        .expect("spawn legacy tty cmd session");
        println!("tty cmd spawn returned");

        let writer = spawned.session.writer_sender();
        writer
            .send(b"echo second\n".to_vec())
            .await
            .expect("send second command");
        writer
            .send(b"exit\n".to_vec())
            .await
            .expect("send exit command");
        spawned.session.close_stdin();

        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(15)).await;
        let stdout = String::from_utf8_lossy(&stdout);
        assert_eq!(exit_code, 0, "stdout={stdout:?}");
        assert!(stdout.contains("ready"), "stdout={stdout:?}");
        assert!(stdout.contains("second"), "stdout={stdout:?}");
    });
}
