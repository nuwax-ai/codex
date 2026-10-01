use super::*;
use crate::ConfigLayerEntry;
use crate::ConfigLayerSource;
use crate::ConfigRequirements;
use crate::ConfigRequirementsToml;
use crate::overrides::build_cli_overrides_layer;
use pretty_assertions::assert_eq;
use toml::Value as TomlValue;

fn session_layer(toml_src: &str) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
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

/// The exact seeds the CLI group produces (nuwax_env.rs builds this table).
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
fn seed_shaped_table_passes_when_selected() {
    assert!(validate_env_group_isolation(&stack(vec![session_layer(GROUP_SEEDS)])).is_ok());
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
    let error = validate_env_group_isolation(&stack(vec![conflicting, session_layer(GROUP_SEEDS)]))
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
}

#[test]
fn subkey_cli_overrides_are_foreign_after_the_real_override_merge() {
    // The CLI seed path and a `-c` subkey override targeting the reserved
    // provider go through the real override builder; the merged result
    // carries the foreign key into the effective table.
    let seed_table: TomlValue = toml::from_str(
        r#"
name = "nuwax env provider"
base_url = "https://gateway.example/v1"
wire_api = "chat"
env_key = "NUWAX_API_KEY"
"#,
    )
    .expect("seed table");
    let cli_layer = build_cli_overrides_layer(&[
        ("model_providers.nuwax_env".to_string(), seed_table),
        (
            "model_providers.nuwax_env.query_params".to_string(),
            TomlValue::Table(
                [("tenant".to_string(), TomlValue::String("t1".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        ),
        (
            "model_provider".to_string(),
            TomlValue::String("nuwax_env".to_string()),
        ),
    ]);
    let error = validate_env_group_isolation(&stack(vec![ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        cli_layer,
    )]))
    .expect_err("subkey overrides are config contributions too");
    assert!(error.to_string().contains("query_params"), "{}", error);
}

#[test]
fn unselected_reserved_provider_with_foreign_keys_passes() {
    // A stale reserved-id definition that no request can reach stays a
    // documented boundary, not a load error: the explicit selection wins,
    // so the seeds add their provider table without selecting it.
    let layers = vec![
        user_layer(
            r#"
model_provider = "openai"

[model_providers.nuwax_env]
name = "unused"
http_headers = { x-old = "1" }
"#,
        ),
        session_layer(
            r#"
model = "env-model"

[model_providers.nuwax_env]
name = "nuwax env provider"
base_url = "https://gateway.example/v1"
wire_api = "chat"
env_key = "NUWAX_API_KEY"
"#,
        ),
    ];
    assert!(validate_env_group_isolation(&stack(layers)).is_ok());
}

#[test]
fn selected_without_a_provider_table_passes() {
    let error_is_expected_downstream = session_layer(r#"model_provider = "nuwax_env""#);
    assert!(validate_env_group_isolation(&stack(vec![error_is_expected_downstream])).is_ok());
}

#[test]
fn user_crafted_four_key_definition_matches_the_group_shape() {
    // A hand-written definition with exactly the seed keys is
    // indistinguishable from (and equivalent to) the group's own seeds.
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
    assert!(validate_env_group_isolation(&stack(vec![crafted])).is_ok());
}

#[test]
fn requirements_selected_reserved_provider_is_checked() {
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
        validate_env_group_isolation(&stack)
            .expect_err("requirement-selected provider is isolated too")
            .to_string()
            .contains("http_headers")
    );
}
