use super::*;
use anyhow::Result;
use codex_app_server_protocol::ConfigEdit;
use codex_app_server_protocol::ConfigLayerSource as ApiConfigLayerSource;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_config::test_support::CloudConfigBundleFixture;
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::sync::Arc;

const ENV_PROVIDER: &str = r#"
name = "Environment provider"
base_url = "https://environment.example/v1"
wire_api = "chat"
env_key = "NUWAX_API_KEY"
"#;

fn seeded_manager(
    home: &Path,
    cli_overrides: Vec<(String, TomlValue)>,
    cloud_config_bundle: CloudConfigBundleLoader,
) -> Result<ConfigManager> {
    Ok(ConfigManager::new(
        home.to_path_buf(),
        cli_overrides,
        vec![
            (
                "model_providers.nuwax_env".to_string(),
                toml::from_str(ENV_PROVIDER)?,
            ),
            (
                "model_provider".to_string(),
                TomlValue::String("nuwax_env".to_string()),
            ),
            (
                "model".to_string(),
                TomlValue::String("environment-model".to_string()),
            ),
        ],
        LoaderOverrides::without_managed_config_for_tests(),
        /*strict_config*/ false,
        cloud_config_bundle,
        codex_arg0::Arg0DispatchPaths::default(),
        Arc::new(codex_config::NoopThreadConfigLoader),
    ))
}

#[tokio::test]
async fn reserved_provider_write_rejects_foreign_fields_before_persistence() -> Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join(CONFIG_TOML_FILE);
    let initial = "# retain this comment\nmodel = 'user-model'\n";
    std::fs::write(&path, initial)?;
    let service = seeded_manager(home.path(), Vec::new(), CloudConfigBundleLoader::default())?;

    for value in [
        serde_json::json!({ "name": "foreign-name-sentinel" }),
        serde_json::json!({
            "name": "foreign-name-sentinel",
            "base_url": "https://foreign-url-sentinel.example/v1",
            "wire_api": "responses",
            "env_key": "FOREIGN_KEY_SENTINEL",
        }),
    ] {
        let error = service
            .write_value(ConfigValueWriteParams {
                file_path: None,
                key_path: "model_providers.nuwax_env".to_string(),
                value,
                merge_strategy: MergeStrategy::Replace,
                expected_version: None,
            })
            .await
            .expect_err("even legal provider fields must come from the environment seed");
        assert_eq!(
            error.write_error_code(),
            Some(ConfigWriteErrorCode::ConfigValidationError)
        );
        let message = error.to_string();
        assert!(
            message.contains("user (") && message.contains("nuwax_env"),
            "{message}"
        );
        assert!(message.contains("name"), "{message}");
        for sentinel in [
            "foreign-name-sentinel",
            "foreign-url-sentinel",
            "FOREIGN_KEY_SENTINEL",
        ] {
            assert!(!message.contains(sentinel), "{message}");
        }
        assert_eq!(std::fs::read_to_string(&path)?, initial);
    }

    let error = service
        .batch_write(ConfigBatchWriteParams {
            file_path: None,
            expected_version: None,
            reload_user_config: false,
            edits: vec![
                ConfigEdit {
                    key_path: "model".to_string(),
                    value: serde_json::json!("changed-model"),
                    merge_strategy: MergeStrategy::Replace,
                },
                ConfigEdit {
                    key_path: "model_providers.nuwax_env".to_string(),
                    value: serde_json::json!({ "name": "foreign-name-sentinel" }),
                    merge_strategy: MergeStrategy::Upsert,
                },
            ],
        })
        .await
        .expect_err("a rejected provider edit must reject the entire batch");
    assert_eq!(
        error.write_error_code(),
        Some(ConfigWriteErrorCode::ConfigValidationError)
    );
    assert!(
        error
            .to_string()
            .contains("only the NUWAX_* environment group")
    );
    assert_eq!(std::fs::read_to_string(&path)?, initial);
    Ok(())
}

#[tokio::test]
async fn reserved_provider_write_checks_final_required_selection() -> Result<()> {
    for required_provider in ["nuwax_env", "openai"] {
        let home = tempfile::tempdir()?;
        let path = home.path().join(CONFIG_TOML_FILE);
        let initial = "model = 'user-model'\n";
        std::fs::write(&path, initial)?;
        let cli_provider = if required_provider == "nuwax_env" {
            "openai"
        } else {
            "nuwax_env"
        };
        let service = seeded_manager(
            home.path(),
            vec![(
                "model_provider".to_string(),
                TomlValue::String(cli_provider.to_string()),
            )],
            CloudConfigBundleFixture::loader_with_enterprise_requirement(format!(
                "model_provider = '{required_provider}'\n"
            )),
        )?;
        let layers = service.load_config_layers(/*cwd*/ None).await?;
        assert_eq!(layers.required_model_provider(), Some(required_provider));
        assert_eq!(
            layers.effective_config()["model_provider"].as_str(),
            Some(cli_provider)
        );

        let result = service
            .write_value(ConfigValueWriteParams {
                file_path: None,
                key_path: "model_providers.nuwax_env".to_string(),
                value: serde_json::json!({ "name": "unselected-user-provider" }),
                merge_strategy: MergeStrategy::Replace,
                expected_version: None,
            })
            .await;
        if required_provider == "nuwax_env" {
            let error =
                result.expect_err("requirements select the reserved provider over CLI flags");
            assert_eq!(
                error.write_error_code(),
                Some(ConfigWriteErrorCode::ConfigValidationError)
            );
            assert!(
                error
                    .to_string()
                    .contains("only the NUWAX_* environment group")
            );
            assert_eq!(std::fs::read_to_string(&path)?, initial);
        } else {
            result?;
            let persisted: TomlValue = toml::from_str(&std::fs::read_to_string(&path)?)?;
            assert_eq!(
                persisted,
                toml::from_str::<TomlValue>(
                    "model = 'user-model'\n[model_providers.nuwax_env]\nname = 'unselected-user-provider'\n"
                )?
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn reserved_provider_cleanup_removes_a_bad_user_contribution() -> Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join(CONFIG_TOML_FILE);
    std::fs::write(
        &path,
        "model = 'user-model'\n[model_providers.nuwax_env]\nname = 'stale-user-provider'\nbase_url = 'https://stale.example/v1'\n",
    )?;
    let service = seeded_manager(home.path(), Vec::new(), CloudConfigBundleLoader::default())?;
    let layers = service.load_config_layers(/*cwd*/ None).await?;
    assert!(
        codex_config::env_group_isolation::validate_env_group_isolation(&layers, "nuwax_env")
            .is_err()
    );

    service
        .write_value(ConfigValueWriteParams {
            file_path: None,
            key_path: "model_providers.nuwax_env".to_string(),
            value: JsonValue::Null,
            merge_strategy: MergeStrategy::Replace,
            expected_version: None,
        })
        .await?;
    assert_eq!(
        toml::from_str::<TomlValue>(&std::fs::read_to_string(&path)?)?,
        toml::from_str::<TomlValue>("model = 'user-model'\n")?
    );
    let layers = service.load_config_layers(/*cwd*/ None).await?;
    codex_config::env_group_isolation::validate_env_group_isolation(&layers, "nuwax_env")?;
    let response = service
        .read(ConfigReadParams {
            include_layers: false,
            cwd: None,
        })
        .await?;
    assert_eq!(response.config.model_provider.as_deref(), Some("nuwax_env"));
    assert_eq!(response.config.model.as_deref(), Some("environment-model"));
    Ok(())
}

#[tokio::test]
async fn reserved_selection_write_without_environment_seed_is_rejected() -> Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join(CONFIG_TOML_FILE);
    let initial = "model = 'user-model'\n";
    std::fs::write(&path, initial)?;
    let service = ConfigManager::without_managed_config_for_tests(home.path().to_path_buf());
    let error = service
        .write_value(ConfigValueWriteParams {
            file_path: None,
            key_path: "model_provider".to_string(),
            value: serde_json::json!("nuwax_env"),
            merge_strategy: MergeStrategy::Replace,
            expected_version: None,
        })
        .await
        .expect_err("selecting the reserved provider requires an actual environment seed");
    assert_eq!(
        error.write_error_code(),
        Some(ConfigWriteErrorCode::ConfigValidationError)
    );
    assert!(error.to_string().contains("not active in this process"));
    assert_eq!(std::fs::read_to_string(&path)?, initial);
    Ok(())
}

// Strict pre-EnvSeed wire clients know only these sources emitted by this fixture.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum LegacyConfigLayerSource {
    SessionFlags,
    User {
        file: AbsolutePathBuf,
        profile: Option<String>,
    },
    System {
        file: AbsolutePathBuf,
    },
}

#[tokio::test]
async fn config_read_keeps_environment_origins_compatible_with_existing_clients() -> Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join(CONFIG_TOML_FILE);
    std::fs::write(&path, "model = 'user-model'\n")?;
    let service = seeded_manager(home.path(), Vec::new(), CloudConfigBundleLoader::default())?;
    let layers = service.load_config_layers(/*cwd*/ None).await?;
    let environment_layer = layers
        .layers_low_to_high()
        .find(|layer| matches!(layer.name, ConfigLayerSource::EnvSeed))
        .expect("the real loader must preserve internal environment provenance");
    let system_file = layers
        .layers_low_to_high()
        .find_map(|layer| match &layer.name {
            ConfigLayerSource::System { file } => Some(file.clone()),
            _ => None,
        })
        .expect("the real loader includes an empty system layer");
    assert_eq!(
        layers.origins()["model_provider"].name,
        ConfigLayerSource::EnvSeed
    );
    codex_config::env_group_isolation::validate_env_group_isolation(&layers, "nuwax_env")?;

    for include_layers in [false, true] {
        let response = service
            .read(ConfigReadParams {
                include_layers,
                cwd: None,
            })
            .await?;
        assert_eq!(response.config.model_provider.as_deref(), Some("nuwax_env"));
        let wire = serde_json::to_value(&response)?;
        let origins = wire["origins"].as_object().expect("serialized origins");
        for metadata in origins.values() {
            serde_json::from_value::<LegacyConfigLayerSource>(metadata["name"].clone())?;
        }
        for key in [
            "model",
            "model_provider",
            "model_providers.nuwax_env.base_url",
        ] {
            assert_eq!(
                response.origins[key].name,
                ApiConfigLayerSource::SessionFlags
            );
            assert_eq!(
                serde_json::from_value::<LegacyConfigLayerSource>(origins[key]["name"].clone())?,
                LegacyConfigLayerSource::SessionFlags
            );
        }
        if include_layers {
            let legacy_names = wire["layers"]
                .as_array()
                .expect("requested config layers")
                .iter()
                .map(|layer| {
                    serde_json::from_value::<LegacyConfigLayerSource>(layer["name"].clone())
                })
                .collect::<serde_json::Result<Vec<_>>>()?;
            assert_eq!(
                legacy_names,
                vec![
                    LegacyConfigLayerSource::SessionFlags,
                    LegacyConfigLayerSource::User {
                        file: AbsolutePathBuf::try_from(path.clone())?,
                        profile: None,
                    },
                    LegacyConfigLayerSource::System {
                        file: system_file.clone()
                    },
                ]
            );
            let api_environment_layer = &response.layers.as_ref().expect("requested layers")[0];
            assert_eq!(
                api_environment_layer,
                &codex_app_server_protocol::ConfigLayer {
                    name: ApiConfigLayerSource::SessionFlags,
                    version: environment_layer.version.clone(),
                    config: serde_json::to_value(&environment_layer.config)?,
                    disabled_reason: None,
                }
            );
        } else {
            assert_eq!(response.layers, None);
            assert!(wire.get("layers").is_none());
        }
    }
    Ok(())
}
