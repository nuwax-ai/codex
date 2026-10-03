//! Exercise provider defaults and explicit choices through real app-server requests.

use super::*;
use crate::legacy_core::config::ConfigBuilder;
use crate::legacy_core::config::ConfigOverrides;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn starts_and_forks_use_server_provider_unless_explicitly_selected() -> Result<()> {
    let home = tempfile::tempdir()?;
    std::fs::write(
        home.path().join("config.toml"),
        r#"
model_provider = "server-provider"
model = "gpt-5.2"
[model_providers.server-provider]
name = "Server provider"
base_url = "http://127.0.0.1:9/v1"
wire_api = "responses"
requires_openai_auth = false
[model_providers.client-provider]
name = "Client provider"
base_url = "http://127.0.0.1:9/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
    )?;
    let server_config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .build()
        .await?;
    let client_config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .harness_overrides(ConfigOverrides {
            model_provider: Some("client-provider".into()),
            ..Default::default()
        })
        .build()
        .await?;
    let local_settings = LocalSettings::from(&client_config);
    let parent = crate::tests::write_session_rollout(
        home.path(),
        "2025-01-02T10-00-00",
        "2025-01-02T10:00:00Z",
        "Server history",
        "server-provider",
        server_config.cwd.as_path(),
    )?;
    let mut server = crate::start_embedded_app_server_for_picker(&server_config).await?;
    assert_eq!(
        crate::lookup_session_target_with_app_server(&mut server, &client_config, "Server history")
            .await?
            .unwrap()
            .thread_id,
        parent
    );
    let resumed = server
        .resume_thread(
            &local_settings,
            client_config.clone(),
            parent,
            ResumeModelSettings::OverrideFromCurrentConfig,
        )
        .await?;
    assert_eq!(resumed.session.model_provider_id, "server-provider");
    for explicit in [false, true] {
        server.model_provider_override = explicit.then(|| "client-provider".to_string());
        let expected = if explicit {
            "client-provider"
        } else {
            "server-provider"
        };
        let started = server.start_thread(&client_config).await?;
        let forked = server
            .fork_thread(&local_settings, client_config.clone(), parent)
            .await?;
        assert_eq!(
            (
                started.session.model_provider_id.as_str(),
                forked.session.model_provider_id.as_str()
            ),
            (expected, expected)
        );
    }
    server.model_provider_override = None;
    let path = home.path().join("config.toml");
    let updated = std::fs::read_to_string(&path)?.replacen(
        "model_provider = \"server-provider\"",
        "model_provider = \"client-provider\"",
        1,
    );
    std::fs::write(path, updated)?;
    assert_eq!(
        server.history_model_provider(&client_config).await?,
        Some("client-provider".into())
    );
    assert_eq!(
        server
            .start_thread(&client_config)
            .await?
            .session
            .model_provider_id,
        "client-provider"
    );
    // A fork of the active session preserves its provider even if launch defaults differ.
    server.model_provider_override = Some("client-provider".into());
    let forked = server
        .fork_thread_at(
            &local_settings,
            server_config,
            parent,
            /*last_turn_id*/ None,
            /*before_turn_id*/ None,
            ForkGoalContinuation::StartIfIdle,
            /*selected_profile*/ None,
        )
        .await?;
    assert_eq!(forked.session.model_provider_id, "server-provider");
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn selected_profile_provider_is_explicit() -> Result<()> {
    let home = tempfile::tempdir()?;
    let profile_path = AbsolutePathBuf::try_from(home.path().join("selected.config.toml"))?;
    std::fs::write(&profile_path, "model_provider = \"openai\"\n")?;
    let config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .loader_overrides(codex_config::LoaderOverrides {
            user_config_path: Some(profile_path),
            user_config_profile: Some("selected".parse()?),
            ..codex_config::LoaderOverrides::without_managed_config_for_tests()
        })
        .build()
        .await?;
    assert_eq!(
        provider_selection::explicit_provider(&config),
        Some("openai".into())
    );
    Ok(())
}

#[tokio::test]
async fn required_provider_overrides_oss_history_selection() -> Result<()> {
    let home = tempfile::tempdir()?;
    std::fs::write(
        home.path().join("requirements.toml"),
        "model_provider = 'openai'",
    )?;
    let config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .loader_overrides(
            codex_config::LoaderOverrides::with_managed_config_path_for_tests(
                home.path().join("managed_config.toml"),
            ),
        )
        .harness_overrides(ConfigOverrides {
            model_provider: Some("ollama".into()),
            ..Default::default()
        })
        .build()
        .await?;
    let mut server = crate::start_embedded_app_server_for_picker(&config).await?;
    server.model_provider_override = Some("ollama".into());
    assert_eq!(config.model_provider_id, "openai");
    assert_eq!(
        server.history_model_provider(&config).await?,
        Some("openai".into())
    );
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn environment_provider_is_explicit_in_remote_start_fork_resume_and_history() -> Result<()> {
    let home = tempfile::tempdir()?;
    let seeds = codex_utils_cli::nuwax_env_overrides(
        codex_utils_cli::NuwaxEnvInput {
            model: Some("environment-model".into()),
            base_url: Some("https://gateway.example/v1".into()),
            wire_api: Some("chat".into()),
            api_key: Some("test-key".into()),
            ..Default::default()
        },
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .map_err(color_eyre::eyre::Report::msg)?;
    let server_overrides = vec![(
        "model_provider".into(),
        toml::Value::String("openai".into()),
    )];
    let loader_overrides = codex_config::LoaderOverrides::without_managed_config_for_tests();
    let server_config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .cli_overrides(server_overrides.clone())
        .env_seed_overrides(seeds.clone())
        .loader_overrides(loader_overrides.clone())
        .build()
        .await?;
    let client_config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .env_seed_overrides(seeds.clone())
        .loader_overrides(loader_overrides.clone())
        .build()
        .await?;
    let parent = crate::tests::write_session_rollout(
        home.path(),
        "2025-01-02T10-00-00",
        "2025-01-02T10:00:00Z",
        "OpenAI history",
        "openai",
        server_config.cwd.as_path(),
    )?;
    let state_db = codex_rollout::state_db::try_init(&server_config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    let client = crate::start_embedded_app_server(
        codex_arg0::Arg0DispatchPaths::default(),
        server_config,
        server_overrides,
        seeds,
        loader_overrides,
        /*strict_config*/ false,
        codex_config::CloudConfigBundleLoader::default(),
        codex_feedback::CodexFeedback::new(),
        /*log_db*/ None,
        Some(state_db),
        Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
        Default::default(),
    )
    .await?;
    let mut server =
        AppServerSession::new(AppServerClient::InProcess(client), ThreadParamsMode::Remote);
    assert_eq!(
        server.history_model_provider(&client_config).await?,
        Some("nuwax_env".into()),
    );
    let local_settings = LocalSettings::from(&client_config);
    let started = server.start_thread(&client_config).await?;
    let forked = server
        .fork_thread(&local_settings, client_config.clone(), parent)
        .await?;
    let resumed = server
        .resume_thread(
            &local_settings,
            client_config,
            parent,
            ResumeModelSettings::OverrideFromCurrentConfig,
        )
        .await?;
    assert_eq!(
        (
            started.session.model_provider_id,
            forked.session.model_provider_id,
            resumed.session.model_provider_id,
            resumed.session.model,
        ),
        (
            "nuwax_env".to_string(),
            "nuwax_env".to_string(),
            "nuwax_env".to_string(),
            "environment-model".to_string(),
        ),
    );
    server.shutdown().await?;
    Ok(())
}
