use super::*;
use crate::ConfigLayerEntry;
use crate::ConfigLayerSource;
use crate::ConfigRequirements;
use crate::ConfigRequirementsToml;
use pretty_assertions::assert_eq;

fn session_layer(toml_src: &str) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str(toml_src).expect("layer toml"),
    )
}

fn env_seed_layer(toml_src: &str) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::EnvSeed,
        toml::from_str(toml_src).expect("layer toml"),
    )
}

fn user_layer(toml_src: &str) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: crate::AbsolutePathBuf::try_from("/tmp/config.toml".to_string()).expect("path"),
            profile: None,
        },
        toml::from_str(toml_src).expect("layer toml"),
    )
}

fn stack(layers: Vec<ConfigLayerEntry>) -> ConfigLayerStack {
    ConfigLayerStack::new(
        layers,
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("layer stack")
}

/// The exact seeds the environment group produces: `nuwax_env.rs` builds
/// this table and rides it on the EnvSeed layer.
const GROUP_SEEDS: &str = r#"
model_provider = "nuwax_env"
model = "env-model"

[model_providers.nuwax_env]
name = "nuwax env provider"
base_url = "https://gateway.example/v1"
wire_api = "chat"
env_key = "NUWAX_API_KEY"
"#;

#[test]
fn seed_layer_passes_when_selected() {
    assert!(
        validate_env_group_isolation(
            &stack(vec![env_seed_layer(GROUP_SEEDS)]),
            NUWAX_ENV_PROVIDER_ID,
        )
        .is_ok()
    );
}

#[test]
fn seeds_without_the_provider_table_do_not_create_it() {
    // Model-only seeds (NUWAX_MODEL without the group) must not satisfy the
    // isolation when something else selects the reserved id.
    let error = validate_env_group_isolation(
        &stack(vec![
            user_layer(r#"model_provider = "nuwax_env""#),
            env_seed_layer(r#"model = "env-model""#),
        ]),
        NUWAX_ENV_PROVIDER_ID,
    )
    .expect_err("the reserved provider exists only through the group");
    assert!(error.to_string().contains("not active"), "{}", error);
}

#[test]
fn reserved_selection_without_any_seed_fails() {
    // A file-defined four-key table used to pass the key-name whitelist;
    // origin matters: selecting the reserved id without the environment
    // group is a hard error.
    let crafted = user_layer(
        r#"
model_provider = "nuwax_env"

[model_providers.nuwax_env]
name = "crafted"
base_url = "https://crafted.example/v1"
wire_api = "responses"
env_key = "NUWAX_API_KEY"
"#,
    );
    let error = validate_env_group_isolation(&stack(vec![crafted]), NUWAX_ENV_PROVIDER_ID)
        .expect_err("user-crafted tables cannot impersonate the group");
    assert!(error.to_string().contains("not active"), "{}", error);
}

#[test]
fn foreign_keys_from_config_files_fail_naming_keys_not_values() {
    let conflicting = user_layer(
        r#"
[model_providers.nuwax_env]
name = "stale definition"
http_headers = { x-old-gateway = "credential-looking-value" }
aws = { region = "us-east-1" }
"#,
    );
    let error = validate_env_group_isolation(
        &stack(vec![conflicting, env_seed_layer(GROUP_SEEDS)]),
        NUWAX_ENV_PROVIDER_ID,
    )
    .expect_err("foreign fields must fail the load");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    let message = error.to_string();
    for key in ["http_headers", "aws"] {
        assert!(message.contains(key), "{message}");
    }
    assert!(
        !message.contains("credential-looking-value") && !message.contains("us-east-1"),
        "the error must name keys, never values: {message}"
    );
    assert!(message.contains("nuwax_env"), "{message}");
    assert!(
        message.contains("user"),
        "the error names the layer: {message}"
    );
}

#[test]
fn same_named_legal_keys_from_a_file_fail_too() {
    // The N1 hole: a file re-defining base_url or env_key with legal key
    // names used to merge silently over the seeds, redirecting the
    // credential to another endpoint.
    for key in ["base_url", "env_key", "wire_api", "name"] {
        let conflicting = user_layer(&format!(
            r#"
[model_providers.nuwax_env]
{key} = "file-supplied-value"
"#
        ));
        let error = validate_env_group_isolation(
            &stack(vec![conflicting, env_seed_layer(GROUP_SEEDS)]),
            NUWAX_ENV_PROVIDER_ID,
        )
        .expect_err("same-named legal keys are foreign contributions");
        let message = error.to_string();
        assert!(message.contains(key), "{key}: {message}");
        assert!(
            !message.contains("file-supplied-value"),
            "values never surface: {message}"
        );
    }
}

#[test]
fn session_layer_subkeys_fail_even_with_legal_names() {
    // Thread config / `-c` layers land on SessionFlags; with the seed on its
    // own layer their contributions stay attributable and are rejected.
    let session = session_layer(
        r#"
[model_providers.nuwax_env.env_key]
variable = "OLD_AUTH_KEY"
"#,
    );
    let error = validate_env_group_isolation(
        &stack(vec![env_seed_layer(GROUP_SEEDS), session]),
        NUWAX_ENV_PROVIDER_ID,
    )
    .expect_err("session contributions to the reserved table are foreign");
    assert!(error.to_string().contains("env_key"), "{}", error);
}

#[test]
fn unselected_reserved_provider_with_foreign_keys_passes() {
    // A stale reserved-id definition that no request can reach stays a
    // documented boundary, not a load error: the explicit selection wins.
    let layers = vec![
        user_layer(
            r#"
model_provider = "openai"

[model_providers.nuwax_env]
name = "unused"
http_headers = { x-old = "1" }
"#,
        ),
        env_seed_layer(GROUP_SEEDS),
    ];
    assert!(validate_env_group_isolation(&stack(layers), "openai").is_ok());
}

#[test]
fn requirements_selecting_the_reserved_provider_still_need_the_seed() {
    let requirements_toml = ConfigRequirementsToml {
        model_provider: Some(NUWAX_ENV_PROVIDER_ID.to_string()),
        ..Default::default()
    };
    let stack = ConfigLayerStack::new(
        vec![user_layer(
            r#"
[model_providers.nuwax_env]
name = "stale"
http_headers = { x-old = "1" }
"#,
        )],
        ConfigRequirements::default(),
        requirements_toml,
    )
    .expect("layer stack");
    assert!(
        validate_env_group_isolation(&stack, NUWAX_ENV_PROVIDER_ID)
            .expect_err("requirement-selected provider is isolated too")
            .to_string()
            .contains("not active")
    );
}

#[test]
fn requirements_defining_the_reserved_table_fail() {
    // Managed requirements replace complete provider entries; the reserved
    // id must not be definable there either.
    let provider = codex_model_provider_info::ModelProviderInfo {
        name: "managed".to_string(),
        base_url: Some("https://managed.example/v1".to_string()),
        env_key: Some("MANAGED_KEY".to_string()),
        ..Default::default()
    };
    let requirements_toml = ConfigRequirementsToml {
        model_providers: Some(
            [(NUWAX_ENV_PROVIDER_ID.to_string(), provider)]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    };
    let stack = ConfigLayerStack::new(
        vec![env_seed_layer(GROUP_SEEDS)],
        ConfigRequirements::default(),
        requirements_toml,
    )
    .expect("layer stack");
    assert!(
        validate_env_group_isolation(&stack, NUWAX_ENV_PROVIDER_ID)
            .expect_err("managed requirements cannot define the reserved provider")
            .to_string()
            .contains("managed requirements")
    );
}
