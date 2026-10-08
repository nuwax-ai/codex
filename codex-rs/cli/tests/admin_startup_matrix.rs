//! B3 acceptance: the administrative session commands (archive, unarchive,
//! delete) through the REAL CLI across the observable startup matrix —
//! install method × daemon presence × NUWAX environment state — plus the
//! queue delivery to a live owner and cross-provider name
//! ambiguity for administrative targeting.
//!
//! Observables are real end state: the rollout file moves to
//! `archived_sessions/` (and back), the daemon stays usable on its socket,
//! config.toml bytes never change, and the owner reports the exact queued
//! message and submission ID returned by the CLI.

use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_fake_rollout;
use codex_app_server::app_server_control_socket_path;
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::RemoteAppServerClient;
use codex_app_server_client::RemoteAppServerConnectArgs;
use codex_app_server_client::RemoteAppServerEndpoint;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::QueuedSubmission;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadQueueListParams;
use codex_app_server_protocol::ThreadQueueListResponse;
use codex_app_server_protocol::UserInput;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use codex_protocol::shell_environment::OPENAI_FEDERATION_RULE_ID_ENV_VAR;
use codex_protocol::shell_environment::OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR;
use pretty_assertions::assert_eq;

fn socket_test_home() -> Result<tempfile::TempDir> {
    // macOS temporary paths can exceed the Unix socket path limit.
    #[cfg(target_os = "macos")]
    let home = tempfile::tempdir_in("/tmp")?;
    #[cfg(not(target_os = "macos"))]
    let home = tempfile::TempDir::new()?;
    Ok(home)
}

/// Environment scrub mirroring queue_owner_dispatch: inherited developer
/// settings (reasoning budgets, install markers, NUWAX/OpenAI auth) must not
/// leak into either side of any matrix cell.
fn cleaned_environment() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("CODEX_SQLITE_HOME", None),
        ("CODEX_MODEL_REASONING_EFFORT", None),
        ("CODEX_MODEL_CONTEXT_WINDOW", None),
        ("CODEX_AUTO_COMPACT_TOKEN_LIMIT", None),
        ("CODEX_AUTO_COMPACT_RATIO", None),
        ("CODEX_MANAGED_BY_VITE_PLUS", None),
        ("CODEX_MANAGED_BY_PNPM", None),
        ("CODEX_MANAGED_BY_NPM", None),
        ("CODEX_MANAGED_BY_BUN", None),
        ("CODEX_INSTALL_SOURCE", None),
        ("NUWAX_MODEL", None),
        ("NUWAX_BASE_URL", None),
        ("NUWAX_WIRE_API", None),
        ("NUWAX_API_KEY", None),
        ("NUWAX_MAX_OUTPUT_TOKENS", None),
        ("NUWAX_REQUEST_MAX_RETRIES", None),
        ("NUWAX_STREAM_MAX_RETRIES", None),
        ("NUWAX_STREAM_IDLE_TIMEOUT_MS", None),
        ("OPENAI_API_KEY", None),
        ("CODEX_API_KEY", None),
        ("CODEX_ACCESS_TOKEN", None),
        (OPENAI_FEDERATION_RULE_ID_ENV_VAR, None),
        (OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR, None),
    ]
}

async fn wait_for_socket(
    home: &std::path::Path,
) -> Result<codex_utils_absolute_path::AbsolutePathBuf> {
    let socket_path = app_server_control_socket_path(home)?;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 30), async {
        while !socket_path.as_path().try_exists()? {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(socket_path)
}

async fn connect_owner(
    socket_path: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<RemoteAppServerClient> {
    Ok(RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
        endpoint: RemoteAppServerEndpoint::UnixSocket {
            socket_path: socket_path.clone(),
        },
        client_name: "admin-startup-matrix-test".to_string(),
        client_version: "0.1.0".to_string(),
        experimental_api: true,
        mcp_server_openai_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?)
}

/// Where the rollout file for a thread currently lives under the home tree.
fn find_rollout_file(home: &std::path::Path, thread_id: &str) -> Option<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, thread_id: &str) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path, thread_id) {
                    return Some(found);
                }
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(thread_id))
            {
                return Some(path);
            }
        }
        None
    }
    walk(home, thread_id)
}

struct AdminCli {
    codex: std::path::PathBuf,
    home: tempfile::TempDir,
    config_before: Vec<u8>,
}

impl AdminCli {
    fn new() -> Result<Self> {
        let home = socket_test_home()?;
        std::fs::write(
            home.path().join("config.toml"),
            "features.plugins = false\nanalytics.enabled = false\n",
        )?;
        let config_before = std::fs::read(home.path().join("config.toml"))?;
        Ok(Self {
            codex: codex_utils_cargo_bin::cargo_bin("codex")?,
            home,
            config_before,
        })
    }

    /// Runs `codex <args>` with the scrubbed environment plus optional
    /// overrides (env pairs, install source).
    async fn run(
        &self,
        args: &[&str],
        env_overrides: &[(&str, &str)],
        install_source: Option<&str>,
    ) -> Result<std::process::Output> {
        let mut command = tokio::process::Command::new(&self.codex);
        for (variable, _) in cleaned_environment() {
            command.env_remove(variable);
        }
        command.env("CODEX_HOME", self.home.path());
        command.current_dir(self.home.path());
        if let Some(source) = install_source {
            command.env("CODEX_INSTALL_SOURCE", source);
        }
        for (key, value) in env_overrides {
            command.env(key, value);
        }
        command.args(args).kill_on_drop(true);
        Ok(
            tokio::time::timeout(Duration::from_secs(/*secs*/ 60), command.output())
                .await
                .context("administrative command did not finish")??,
        )
    }

    async fn spawn_envless_daemon(
        &self,
    ) -> Result<(TestAppServer, codex_utils_absolute_path::AbsolutePathBuf)> {
        let server = TestAppServer::builder()
            .with_program(&self.codex)
            .with_codex_home(self.home.path())
            .with_plugin_startup_tasks()
            .without_managed_config()
            .with_args(&["app-server", "--listen", "unix://"])
            .with_env_overrides(&cleaned_environment())
            .build()
            .await?;
        let socket_path = wait_for_socket(self.home.path()).await?;
        Ok((server, socket_path))
    }

    fn assert_config_unchanged(&self) -> Result<()> {
        assert_eq!(
            std::fs::read(self.home.path().join("config.toml"))?,
            self.config_before,
            "administrative commands must never rewrite config.toml"
        );
        Ok(())
    }
}

/// The full observable matrix for archive/unarchive/delete: every cell must
/// reach the same end state on disk while the daemon (when present) stays
/// alive and usable, and config bytes never move.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_commands_cover_install_method_and_daemon_presence_matrix() -> Result<()> {
    struct Cell {
        label: &'static str,
        daemon: bool,
        env: bool,
        install_source: Option<&'static str>,
    }
    let cells = [
        Cell {
            label: "default+daemon+envless",
            daemon: true,
            env: false,
            install_source: None,
        },
        Cell {
            label: "default+daemon+env",
            daemon: true,
            env: true,
            install_source: None,
        },
        Cell {
            label: "default+envless+no-daemon",
            daemon: false,
            env: false,
            install_source: None,
        },
        Cell {
            label: "npm+no-daemon",
            daemon: false,
            env: false,
            install_source: Some("npm_nuwax"),
        },
        Cell {
            label: "default+env+no-daemon",
            daemon: false,
            env: true,
            install_source: None,
        },
        Cell {
            label: "npm+env+no-daemon",
            daemon: false,
            env: true,
            install_source: Some("npm_nuwax"),
        },
        Cell {
            label: "npm+daemon+envless",
            daemon: true,
            env: false,
            install_source: Some("npm_nuwax"),
        },
        Cell {
            label: "npm+daemon+env",
            daemon: true,
            env: true,
            install_source: Some("npm_nuwax"),
        },
    ];

    for cell in cells {
        // archive
        let cli = AdminCli::new()?;
        let thread_id = create_fake_rollout(
            cli.home.path(),
            "2025-04-01T01-00-00",
            "2025-04-01T01:00:00Z",
            "matrix archive session",
            Some(NUWAX_ENV_PROVIDER_ID),
            /*git_info*/ None,
        )?;
        let daemon = if cell.daemon {
            Some(cli.spawn_envless_daemon().await?)
        } else {
            None
        };
        let env: &[(&str, &str)] = if cell.env {
            &[
                ("NUWAX_BASE_URL", "https://env.example/v1"),
                ("NUWAX_WIRE_API", "chat"),
                ("NUWAX_API_KEY", "matrix-env-key"),
                ("NUWAX_MODEL", "matrix-env-model"),
            ]
        } else {
            &[]
        };
        let output = cli
            .run(&["archive", &thread_id], env, cell.install_source)
            .await?;
        assert!(
            output.status.success(),
            "{}: archive must succeed: {}",
            cell.label,
            String::from_utf8_lossy(&output.stderr)
        );
        let original = find_rollout_file(cli.home.path(), &thread_id)
            .context(format!("{}: rollout file must still exist", cell.label))?;
        assert!(
            original
                .components()
                .any(|c| c.as_os_str() == "archived_sessions"),
            "{}: archive must move the rollout into archived_sessions/, got {}",
            cell.label,
            original.display()
        );
        cli.assert_config_unchanged()?;

        // unarchive (only meaningful in cells with a daemon to keep proving
        // liveness; all cells exercise the command itself)
        let output = cli
            .run(&["unarchive", &thread_id], env, cell.install_source)
            .await?;
        assert!(
            output.status.success(),
            "{}: unarchive must succeed: {}",
            cell.label,
            String::from_utf8_lossy(&output.stderr)
        );
        let restored = find_rollout_file(cli.home.path(), &thread_id).context(format!(
            "{}: rollout must return from archived_sessions",
            cell.label
        ))?;
        assert!(
            !restored
                .components()
                .any(|c| c.as_os_str() == "archived_sessions"),
            "{}: unarchive must move the rollout back, got {}",
            cell.label,
            restored.display()
        );

        // delete
        let output = cli
            .run(&["delete", &thread_id, "--force"], env, cell.install_source)
            .await?;
        assert!(
            output.status.success(),
            "{}: delete must succeed: {}",
            cell.label,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            find_rollout_file(cli.home.path(), &thread_id).is_none(),
            "{}: delete must remove the rollout",
            cell.label
        );
        cli.assert_config_unchanged()?;

        if let Some((_server, socket)) = &daemon {
            // The daemon must still serve RPCs on its socket afterwards. The
            // matrix target was just deleted, so probe with a surviving
            // thread's queue instead.
            let probe_thread = create_fake_rollout(
                cli.home.path(),
                "2025-04-01T01-30-00",
                "2025-04-01T01:30:00Z",
                "matrix daemon probe session",
                Some(NUWAX_ENV_PROVIDER_ID),
                /*git_info*/ None,
            )?;
            let app = connect_owner(socket).await?;
            let queue: ThreadQueueListResponse = app
                .request_typed(ClientRequest::ThreadQueueList {
                    request_id: RequestId::Integer(7),
                    params: ThreadQueueListParams {
                        thread_id: probe_thread,
                        cursor: None,
                        limit: None,
                    },
                })
                .await?;
            assert_eq!(queue.data.len(), 0);
            app.shutdown().await?;
        }
    }
    Ok(())
}

/// Package-4 slice: `--strict-config` and `--profile` must reach the REAL
/// config loader behind administrative commands, not only the TuiCli merge.
/// Strict-config turns an unrecognized config field into a hard failure with
/// the rollout untouched; profile selection layers the selected profile over
/// config.toml (an unknown provider fails, a valid profile archives normally).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_cli_flags_reach_real_config_loading() -> Result<()> {
    let cli = AdminCli::new()?;
    std::fs::write(
        cli.home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\nnot_a_real_setting = true\n",
    )?;
    let config_with_unknown_field = std::fs::read(cli.home.path().join("config.toml"))?;
    let thread_id = create_fake_rollout(
        cli.home.path(),
        "2025-04-02T02-00-00",
        "2025-04-02T02:00:00Z",
        "flags strict session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;

    // --strict-config: an unrecognized config field is fatal and the rollout
    // must not move.
    let output = cli
        .run(&["archive", &thread_id, "--strict-config"], &[], None)
        .await?;
    assert!(
        !output.status.success(),
        "strict-config must fail on an unrecognized config field: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let unchanged = find_rollout_file(cli.home.path(), &thread_id)
        .context("strict-config failure must leave the rollout in place")?;
    assert!(
        !unchanged
            .components()
            .any(|c| c.as_os_str() == "archived_sessions"),
        "strict-config failure must not archive"
    );
    assert_eq!(
        std::fs::read(cli.home.path().join("config.toml"))?,
        config_with_unknown_field,
        "strict-config failure must not rewrite config.toml"
    );

    // Without the flag the unknown field is ignored and the command archives.
    let output = cli.run(&["archive", &thread_id], &[], None).await?;
    assert!(
        output.status.success(),
        "fallback archive must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let archived = find_rollout_file(cli.home.path(), &thread_id)
        .context("fallback archive must keep the rollout")?;
    assert!(
        archived
            .components()
            .any(|c| c.as_os_str() == "archived_sessions"),
        "fallback archive must move the rollout"
    );
    let output = cli.run(&["unarchive", &thread_id], &[], None).await?;
    assert!(
        output.status.success(),
        "restore for profile cells must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(cli.home.path().join("config.toml"), &cli.config_before)?;
    let restored = find_rollout_file(cli.home.path(), &thread_id)
        .context("restored rollout for profile cells")?;

    // --profile uses the profile-v2 file mechanism (`<name>.config.toml` in
    // CODEX_HOME): a profile whose provider does not exist must fail the
    // command (the profile layer reached the loader), and a valid profile
    // archives normally through the same flag path.
    std::fs::write(
        cli.home.path().join("admin.config.toml"),
        "model_provider = \"no-such-provider\"\n",
    )?;
    let output = cli
        .run(
            &[
                "archive",
                &thread_id,
                "--strict-config",
                "--profile",
                "admin",
            ],
            &[],
            None,
        )
        .await?;
    assert!(
        !output.status.success(),
        "the selected profile's unknown provider must fail the load"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Model provider `no-such-provider` not found"),
        "the failure must come from the selected profile: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        find_rollout_file(cli.home.path(), &thread_id),
        Some(restored),
        "profile load failure must leave the rollout at its original path"
    );
    std::fs::write(
        cli.home.path().join("ok.config.toml"),
        "approval_policy = \"never\"\n",
    )?;
    let output = cli
        .run(&["archive", &thread_id, "--profile", "ok"], &[], None)
        .await?;
    assert!(
        output.status.success(),
        "a valid profile must archive normally: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        find_rollout_file(cli.home.path(), &thread_id).is_some_and(|path| {
            path.components()
                .any(|c| c.as_os_str() == "archived_sessions")
        }),
        "profile-selected archive must move the rollout"
    );
    cli.assert_config_unchanged()?;
    Ok(())
}

/// Package-4 slice: `--oss` routes provider selection for administrative
/// commands. Implicit selection reads `oss_provider` from config.toml;
/// `--local-provider` overrides it, and an unknown explicit provider is a
/// hard load failure that leaves the rollout untouched.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_cli_oss_flag_routes_provider_selection() -> Result<()> {
    let cli = AdminCli::new()?;
    std::fs::write(
        cli.home.path().join("config.toml"),
        "features.plugins = false\nanalytics.enabled = false\noss_provider = \"no-such-implicit-provider\"\n[model_providers.oss-custom]\nname = \"OSS custom\"\nbase_url = \"http://127.0.0.1:9/v1\"\nwire_api = \"responses\"\n",
    )?;
    let config_before = std::fs::read(cli.home.path().join("config.toml"))?;
    let thread_id = create_fake_rollout(
        cli.home.path(),
        "2025-04-03T03-00-00",
        "2025-04-03T03:00:00Z",
        "oss flag session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;
    let original =
        find_rollout_file(cli.home.path(), &thread_id).context("original rollout for OSS cells")?;

    // An invalid config default proves that --oss reaches provider selection.
    let output = cli
        .run(&["archive", &thread_id, "--oss"], &[], None)
        .await?;
    assert!(
        !output.status.success(),
        "--oss with an unknown oss_provider must fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Model provider `no-such-implicit-provider` not found"),
        "--oss must fail because of its config default: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        find_rollout_file(cli.home.path(), &thread_id),
        Some(original.clone()),
        "implicit provider failure must leave the rollout at its original path"
    );

    // An unknown explicit provider is a hard failure and must not move the
    // rollout; the configured provider through the same flag path succeeds.
    let output = cli
        .run(
            &[
                "archive",
                &thread_id,
                "--oss",
                "--local-provider",
                "no-such-provider",
            ],
            &[],
            None,
        )
        .await?;
    assert!(
        !output.status.success(),
        "an unknown --local-provider must fail the load"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Model provider `no-such-provider` not found"),
        "the explicit provider must override the invalid config default: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        find_rollout_file(cli.home.path(), &thread_id),
        Some(original),
        "unknown provider failure must leave the rollout at its original path"
    );
    let output = cli
        .run(
            &[
                "archive",
                &thread_id,
                "--oss",
                "--local-provider",
                "oss-custom",
            ],
            &[],
            None,
        )
        .await?;
    assert!(
        output.status.success(),
        "the configured provider via --local-provider must archive: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        find_rollout_file(cli.home.path(), &thread_id).is_some_and(|path| {
            path.components()
                .any(|c| c.as_os_str() == "archived_sessions")
        }),
        "--local-provider archive must move the rollout"
    );
    assert_eq!(
        std::fs::read(cli.home.path().join("config.toml"))?,
        config_before,
        "OSS flag selection must not rewrite config.toml"
    );
    Ok(())
}

/// A corrupted local group names the variable and leaves the rollout untouched;
/// an explicit `-c model_provider` mask makes the same command succeed.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_corrupted_group_fails_fast_unless_explicitly_masked() -> Result<()> {
    let cli = AdminCli::new()?;
    let thread_id = create_fake_rollout(
        cli.home.path(),
        "2025-04-02T02-00-00",
        "2025-04-02T02:00:00Z",
        "corrupted admin session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;
    let corrupted = [
        ("NUWAX_BASE_URL", "https://client.example/v1"),
        ("NUWAX_WIRE_API", "carrier-pigeon"),
        ("NUWAX_API_KEY", "corrupted-key"),
        ("NUWAX_MODEL", "m"),
    ];
    let original_path =
        find_rollout_file(cli.home.path(), &thread_id).context("original rollout")?;
    let original_bytes = std::fs::read(&original_path)?;
    let output = cli.run(&["archive", &thread_id], &corrupted, None).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "corrupted group must fail: {stderr}"
    );
    assert!(
        stderr.contains("NUWAX_WIRE_API"),
        "must name the variable: {stderr}"
    );
    assert!(
        !stderr.contains("corrupted-key"),
        "must not echo values: {stderr}"
    );
    assert_eq!(
        find_rollout_file(cli.home.path(), &thread_id),
        Some(original_path.clone()),
        "the failed command must not move the rollout"
    );
    assert_eq!(std::fs::read(&original_path)?, original_bytes);
    assert_eq!(
        find_rollout_file(&cli.home.path().join("archived_sessions"), &thread_id),
        None,
        "the failed command must not leave an archived copy"
    );
    cli.assert_config_unchanged()?;

    // The explicit provider mask makes the whole group irrelevant.
    let output = cli
        .run(
            &["-c", "model_provider=openai", "archive", &thread_id],
            &corrupted,
            None,
        )
        .await?;
    assert!(
        output.status.success(),
        "explicit provider mask must ignore the corrupted group: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        find_rollout_file(cli.home.path(), &thread_id)
            .is_some_and(|p| p.components().any(|c| c.as_os_str() == "archived_sessions")),
        "masked archive must still archive"
    );
    cli.assert_config_unchanged()?;
    Ok(())
}

/// An administrative label shared by two threads across providers is
/// ambiguous: archive must refuse and list both candidate thread IDs.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_archive_by_name_rejects_cross_provider_ambiguity() -> Result<()> {
    let cli = AdminCli::new()?;
    let shared = "shared-admin-name";
    let first = create_fake_rollout(
        cli.home.path(),
        "2025-04-03T03-00-00",
        "2025-04-03T03:00:00Z",
        shared,
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;
    let second = create_fake_rollout(
        cli.home.path(),
        "2025-04-03T03-30-00",
        "2025-04-03T03:30:00Z",
        shared,
        /*provider*/ None,
        /*git_info*/ None,
    )?;
    let original_paths = [
        find_rollout_file(cli.home.path(), &first).context("first original rollout")?,
        find_rollout_file(cli.home.path(), &second).context("second original rollout")?,
    ];
    let original_bytes = [
        std::fs::read(&original_paths[0])?,
        std::fs::read(&original_paths[1])?,
    ];
    let output = cli.run(&["archive", shared], &[], None).await?;
    assert!(!output.status.success(), "ambiguous label must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ambig") || stderr.contains("multiple"),
        "the error must describe the ambiguity: {stderr}"
    );
    assert!(
        stderr.contains(first.as_str()) && stderr.contains(second.as_str()),
        "both candidate ids must be listed: {stderr}"
    );
    assert_eq!(
        [
            find_rollout_file(cli.home.path(), &first),
            find_rollout_file(cli.home.path(), &second),
        ],
        original_paths.clone().map(Some),
        "an ambiguous command must not move either rollout"
    );
    assert_eq!(
        [
            std::fs::read(&original_paths[0])?,
            std::fs::read(&original_paths[1])?,
        ],
        original_bytes,
        "an ambiguous command must not rewrite either rollout"
    );
    assert_eq!(
        [
            find_rollout_file(&cli.home.path().join("archived_sessions"), &first),
            find_rollout_file(&cli.home.path().join("archived_sessions"), &second),
        ],
        [None, None],
        "an ambiguous command must not leave archived copies"
    );
    cli.assert_config_unchanged()?;
    Ok(())
}

/// The live owner receives exactly the submission reported by the CLI.
/// Writer lifecycle exclusivity needs separate lock/registration evidence.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_against_a_live_owner_reports_the_received_submission() -> Result<()> {
    let cli = AdminCli::new()?;
    let thread_id = create_fake_rollout(
        cli.home.path(),
        "2025-04-04T04-00-00",
        "2025-04-04T04:00:00Z",
        "owner queue session",
        Some(NUWAX_ENV_PROVIDER_ID),
        /*git_info*/ None,
    )?;
    let (_daemon, socket) = cli.spawn_envless_daemon().await?;
    let app = connect_owner(&socket).await?;
    let output = cli
        .run(
            &[
                "queue",
                "--thread",
                &thread_id,
                "--message",
                "deliver this to the owner",
            ],
            &[],
            None,
        )
        .await?;
    assert!(
        output.status.success(),
        "enqueue must be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    let submission_id = stdout
        .trim()
        .strip_prefix("Queued message ")
        .and_then(|line| line.strip_suffix(&format!(" for thread {thread_id}.")))
        .context("CLI must report the queued submission and thread IDs")?;
    let queue: ThreadQueueListResponse = app
        .request_typed(ClientRequest::ThreadQueueList {
            request_id: RequestId::Integer(11),
            params: ThreadQueueListParams {
                thread_id: thread_id.clone(),
                cursor: None,
                limit: None,
            },
        })
        .await?;
    let [submission] = queue.data.as_slice() else {
        anyhow::bail!("owner must have exactly one queued submission: {queue:?}");
    };
    assert!(!submission.client_user_message_id.is_empty());
    assert_eq!(
        queue,
        ThreadQueueListResponse {
            data: vec![QueuedSubmission {
                id: submission_id.to_string(),
                input: vec![UserInput::Text {
                    text: "deliver this to the owner".to_string(),
                    text_elements: Vec::new(),
                }],
                client_user_message_id: submission.client_user_message_id.clone(),
            }],
            next_cursor: None,
        },
        "the owner must report the exact CLI submission and message"
    );
    cli.assert_config_unchanged()?;
    app.shutdown().await?;
    Ok(())
}
