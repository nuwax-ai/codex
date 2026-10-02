use super::*;
use anyhow::Result;
use codex_config::ConfigLayerSource;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn startup_config_fallback_preserves_environment_provider_seeds() -> Result<()> {
    let home = tempfile::tempdir()?;
    let config_path = home.path().join("config.toml");
    let invalid_config = "[invalid";
    std::fs::write(&config_path, invalid_config)?;
    let seeds = vec![
        (
            "model_providers.nuwax_env".into(),
            toml::from_str(
                "name = 'Environment provider'\nbase_url = 'https://gateway.example/v1'\nwire_api = 'chat'\nenv_key = 'NUWAX_API_KEY'",
            )?,
        ),
        (
            "model_provider".into(),
            TomlValue::String("nuwax_env".into()),
        ),
        (
            "model".into(),
            TomlValue::String("environment-model".into()),
        ),
    ];
    for cli_overrides in [
        Vec::new(),
        vec![(
            "model_provider".into(),
            TomlValue::String("nuwax_env".into()),
        )],
    ] {
        let mut manager = ConfigManager::new_for_tests(
            home.path().to_path_buf(),
            cli_overrides,
            LoaderOverrides::without_managed_config_for_tests(),
            CloudConfigBundleLoader::default(),
        );
        manager.env_seed_overrides = seeds.clone();
        assert!(
            manager
                .load_latest_config(/*fallback_cwd*/ None)
                .await
                .is_err()
        );
        let config = manager.load_startup_config(/*fallback_cwd*/ None).await?;
        assert_eq!(
            (
                config.model_provider_id.as_str(),
                config.model.as_deref(),
                config.model_provider.base_url.as_deref(),
                config.model_provider.env_key.as_deref(),
            ),
            (
                "nuwax_env",
                Some("environment-model"),
                Some("https://gateway.example/v1"),
                Some("NUWAX_API_KEY"),
            ),
        );
        assert!(config.config_layer_stack.layers_high_to_low().any(|layer| {
            layer.name == ConfigLayerSource::EnvSeed
                && layer.config.get("model_providers").is_some()
        }));
        assert_eq!(std::fs::read_to_string(&config_path)?, invalid_config);
    }
    Ok(())
}
