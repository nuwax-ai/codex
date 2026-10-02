use super::resume_model_settings_for_overrides;
use crate::app_server_session::ResumeModelSettings;
use crate::legacy_core::config::ConfigBuilder;
use crate::legacy_core::config::ConfigOverrides;
use codex_config::LoaderOverrides;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn resume_model_settings_honor_environment_seed_selections() -> color_eyre::Result<()> {
    let home = tempfile::tempdir()?;
    std::fs::write(
        home.path().join("config.toml"),
        "model = 'saved-model'\nmodel_provider = 'openai'\nmodel_reasoning_effort = 'low'\n",
    )?;
    for (key, value) in [
        ("model", "environment-model"),
        ("model_provider", "openai"),
        ("model_reasoning_effort", "high"),
    ] {
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .loader_overrides(LoaderOverrides::without_managed_config_for_tests())
            .env_seed_overrides(vec![(key.into(), toml::Value::String(value.into()))])
            .build()
            .await?;
        assert_eq!(
            resume_model_settings_for_overrides(&config, &ConfigOverrides::default()),
            ResumeModelSettings::OverrideFromCurrentConfig,
            "environment seed for {key} must replace saved thread settings",
        );
    }
    let config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .loader_overrides(LoaderOverrides::without_managed_config_for_tests())
        .build()
        .await?;
    assert_eq!(
        resume_model_settings_for_overrides(&config, &ConfigOverrides::default()),
        ResumeModelSettings::RestoreFromThread,
    );
    Ok(())
}
