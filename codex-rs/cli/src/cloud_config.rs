use anyhow::Context;
use anyhow::Result;
use codex_cloud_config::cloud_config_bundle_loader_for_storage;
use codex_config::CloudConfigBundleLoader;
use codex_config::ConfigLoadOptions;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_core::config::LoaderOverrides;
use codex_core::config::bootstrap_auth_config;
use codex_core::config::find_codex_home;
use codex_core::config::load_config_toml_with_layer_stack_and_env_seed;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cli::CliConfigOverrides;

pub(crate) async fn load_config(
    config_overrides: &CliConfigOverrides,
    loader_overrides: LoaderOverrides,
) -> Result<Config> {
    config_builder(
        config_overrides,
        loader_overrides,
        ConfigOverrides::default(),
    )
    .await?
    .build()
    .await
    .context("failed to load configuration")
}

pub(crate) async fn config_builder(
    config_overrides: &CliConfigOverrides,
    loader_overrides: LoaderOverrides,
    harness_overrides: ConfigOverrides,
) -> Result<ConfigBuilder> {
    let cli_overrides = config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    config_builder_from_parsed_overrides(
        cli_overrides,
        /*env_seed_overrides*/ Vec::new(),
        loader_overrides,
        harness_overrides,
    )
    .await
}

pub(crate) async fn config_builder_from_parsed_overrides(
    cli_overrides: Vec<(String, toml::Value)>,
    env_seed_overrides: Vec<(String, toml::Value)>,
    loader_overrides: LoaderOverrides,
    harness_overrides: ConfigOverrides,
) -> Result<ConfigBuilder> {
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;
    let cwd = match harness_overrides.cwd.as_deref() {
        Some(cwd) => AbsolutePathBuf::relative_to_current_dir(cwd),
        None => AbsolutePathBuf::current_dir(),
    }
    .context("failed to resolve current directory")?;
    let bootstrap_config = load_config_toml_with_layer_stack_and_env_seed(
        codex_home.as_path(),
        Some(&cwd),
        cli_overrides.clone(),
        env_seed_overrides.clone(),
        ConfigLoadOptions {
            loader_overrides: loader_overrides.clone(),
            strict_config: false,
            cloud_config_bundle: CloudConfigBundleLoader::default(),
        },
    )
    .await
    .context("failed to load bootstrap configuration")?;
    let cloud_config_bundle = cloud_config_bundle_loader_for_storage(
        bootstrap_auth_config(codex_home.as_path(), &bootstrap_config)
            .context("failed to resolve cloud configuration authentication")?,
        /*enable_codex_api_key_env*/ false,
    )
    .await
    .context("failed to initialize cloud configuration authentication")?;

    Ok(ConfigBuilder::default()
        .codex_home(codex_home.to_path_buf())
        .cli_overrides(cli_overrides)
        .env_seed_overrides(env_seed_overrides)
        .loader_overrides(loader_overrides)
        .harness_overrides(harness_overrides)
        .cloud_config_bundle(cloud_config_bundle)
        .fallback_cwd(Some(cwd.to_path_buf())))
}
