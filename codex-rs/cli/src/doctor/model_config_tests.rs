use super::model_cli_overrides;
use super::model_routing;
use super::routing_sources;
use clap::Parser;
use codex_core::config::ConfigBuilder;
use codex_login::AuthManager;
use codex_tui::Cli as TuiCli;
use codex_utils_cli::CliConfigOverrides;
use codex_utils_cli::NuwaxEnvInput;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn doctor_resolves_environment_provider_and_reports_scope_aware_compact_limits() {
    let interactive = TuiCli::parse_from(["codex"]);
    for (scope, threshold) in [("total", 90000), ("body_after_prefix", 120000)] {
        let home = tempfile::tempdir().expect("temporary Codex home");
        let launch_overrides = model_cli_overrides(
            &CliConfigOverrides {
                raw_overrides: vec![
                    "model_context_window=100000".into(),
                    "model_auto_compact_token_limit=120000".into(),
                    format!("model_auto_compact_token_limit_scope={scope}"),
                ],
            },
            &interactive,
            NuwaxEnvInput {
                model: Some("doctor-model".into()),
                // Credential-free URL; the query still exercises redaction.
                base_url: Some("https://gateway.example:8443/v1?token=secret".into()),
                wire_api: Some("chat".into()),
                api_key: Some("private-key".into()),
                ..Default::default()
            },
        )
        .expect("doctor model overrides");
        // Credentials in the URL userinfo are rejected at seed time and
        // never become an endpoint.
        let rejected = model_cli_overrides(
            &CliConfigOverrides::default(),
            &interactive,
            NuwaxEnvInput {
                base_url: Some("https://user:secret@gateway.example".into()),
                wire_api: Some("chat".into()),
                api_key: Some("private-key".into()),
                ..Default::default()
            },
        )
        .expect_err("userinfo URLs must fail the group");
        assert!(
            !rejected.to_string().contains("secret"),
            "the error must not echo the credential: {rejected}"
        );
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .cli_overrides(launch_overrides.cli_overrides)
            .env_seed_overrides(launch_overrides.env_seed_overrides)
            .build()
            .await
            .expect("doctor effective config");
        assert_eq!(config.model_provider_id, "nuwax_env");
        assert_eq!(config.model.as_deref(), Some("doctor-model"));
        let auth =
            AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false)
                .await
                .expect("local auth manager");
        let sources = model_routing::ModelRoutingSources {
            plain_oss: false,
            provider: model_routing::ProviderSource::NuwaxEnvironmentGroup,
            model: model_routing::ModelSource::NuwaxModelEnvironment,
        };
        let report = model_routing::check(&config, Some(auth), &sources).await;
        for detail in [
            format!("effective auto compact threshold: {threshold}"),
            format!("auto compact limit scope: {scope}"),
            "full usable context window: 95000".to_string(),
            "endpoint (redacted): https://gateway.example:8443".to_string(),
            "provider source: NUWAX_* environment group (temporary, per-run credentials)".into(),
            "model source: NUWAX_MODEL environment".into(),
        ] {
            assert!(
                report.details.iter().any(|value| value == &detail),
                "{detail}"
            );
        }
        assert!(
            !serde_json::to_string(&report)
                .expect("doctor JSON")
                .contains("secret")
        );
    }
}

#[test]
fn doctor_oss_without_a_provider_ignores_full_and_malformed_environment_groups() {
    let interactive = TuiCli::parse_from(["codex", "--oss"]);
    assert!(interactive.oss_provider.is_none());
    let config = CliConfigOverrides::default();
    let expected = config.parse_overrides().expect("explicit overrides");
    for input in [
        NuwaxEnvInput {
            model: Some("environment-model".into()),
            base_url: Some("https://gateway.example".into()),
            wire_api: Some("chat".into()),
            api_key: Some("private-key".into()),
            ..Default::default()
        },
        NuwaxEnvInput {
            model: Some(" ".into()),
            base_url: Some("not a URL".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(
            model_cli_overrides(&config, &interactive, input).expect("OSS ignores unused env"),
            codex_config::LaunchOverrides {
                cli_overrides: expected.clone(),
                env_seed_overrides: Vec::new(),
            }
        );
    }
}

#[tokio::test]
async fn doctor_sources_use_the_final_provider_override_and_adopted_model_seed() {
    use model_routing::ModelSource;
    use model_routing::ProviderSource;
    let interactive = TuiCli::parse_from(["codex"]);
    for (first, last, provider, model) in [
        (
            "openai",
            "nuwax_env",
            ProviderSource::NuwaxEnvironmentGroup,
            ModelSource::NuwaxModelEnvironment,
        ),
        (
            "nuwax_env",
            "openai",
            ProviderSource::CliOverride,
            ModelSource::ConfigLayers,
        ),
    ] {
        let home = tempfile::tempdir().expect("temporary Codex home");
        let explicit = CliConfigOverrides {
            raw_overrides: vec![
                format!("model_provider=\"{first}\""),
                format!("model_provider=\"{last}\""),
            ],
        };
        let input = NuwaxEnvInput {
            model: Some("env-model".into()),
            base_url: Some("https://gateway.example/v1".into()),
            wire_api: Some("chat".into()),
            api_key: Some("private-key".into()),
            ..Default::default()
        };
        let codex_config::LaunchOverrides {
            cli_overrides: explicit_pairs,
            env_seed_overrides: seeds,
        } = model_cli_overrides(&explicit, &interactive, input.clone()).expect("seeds");
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .cli_overrides(explicit_pairs)
            .env_seed_overrides(seeds)
            .build()
            .await
            .expect("effective config");
        assert_eq!(
            routing_sources(&config, &interactive, &explicit),
            model_routing::ModelRoutingSources {
                plain_oss: false,
                provider,
                model
            }
        );
    }
}

#[tokio::test]
async fn doctor_endpoint_source_tracks_required_provider_definitions() -> anyhow::Result<()> {
    for (requirements, origin, endpoint) in [
        (
            "model_provider = 'gateway'",
            "user",
            "https://local.example",
        ),
        (
            "[model_providers.gateway]\nname = 'Managed gateway'\nbase_url = 'https://managed.example/v1'\nwire_api = 'responses'",
            "managed requirements",
            "https://managed.example",
        ),
        (
            "model_provider = 'gateway'\n[model_providers.gateway]\nname = 'Managed gateway'\nbase_url = 'https://managed.example/v1'\nwire_api = 'responses'",
            "managed requirements",
            "https://managed.example",
        ),
        (
            "[model_providers.other]\nname = 'Other managed provider'\nbase_url = 'https://other.example/v1'\nwire_api = 'responses'",
            "user",
            "https://local.example",
        ),
        (
            "[model_providers.gateway]\nname = 'Managed default endpoint'\nwire_api = 'responses'",
            "no base_url override (built-in provider)",
            "<provider default>",
        ),
    ] {
        let home = tempfile::tempdir()?;
        let config_path = home.path().join("config.toml");
        std::fs::write(
            &config_path,
            "model_provider = 'gateway'\n[model_providers.gateway]\nname = 'Local gateway'\nbase_url = 'https://local.example/v1'\nwire_api = 'responses'",
        )?;
        std::fs::write(home.path().join("requirements.toml"), requirements)?;
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .loader_overrides(
                codex_config::LoaderOverrides::with_managed_config_path_for_tests(
                    home.path().join("managed_config.toml"),
                ),
            )
            .build()
            .await?;
        let report = model_routing::check(
            &config,
            /*auth_manager*/ None,
            &model_routing::ModelRoutingSources {
                plain_oss: false,
                provider: model_routing::ProviderSource::ConfigLayers,
                model: model_routing::ModelSource::ConfigLayers,
            },
        )
        .await;
        let endpoint_details = report
            .details
            .into_iter()
            .filter(|detail| detail.starts_with("endpoint "))
            .collect::<Vec<_>>();
        let source = if origin == "user" {
            format!("user ({})", config_path.display())
        } else {
            origin.to_string()
        };
        assert_eq!(
            endpoint_details,
            vec![
                format!("endpoint (redacted): {endpoint}"),
                format!("endpoint source: {source}"),
            ],
            "requirements: {requirements}",
        );
    }
    Ok(())
}
