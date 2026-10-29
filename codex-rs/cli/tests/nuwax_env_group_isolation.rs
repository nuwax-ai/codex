//! The environment group's seeds must survive the reserved-provider
//! isolation check.
//!
//! `env_group_isolation` refuses to load when the reserved provider's table
//! holds a key the group does not seed. A key added to the seeds without
//! being added to that whitelist therefore fails every run with the group
//! active — and no unit test catches it, because none of them runs the real
//! seed producer through the real validator. This one does: it seeds the
//! group from the same function the binary calls, then asks the same check
//! the loader asks.

// Test fixtures should fail immediately when setup is invalid.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirements;
use codex_config::ConfigRequirementsToml;
use codex_config::env_group_isolation::validate_env_group_isolation;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use codex_utils_cli::NuwaxEnvInput;
use codex_utils_cli::nuwax_env_overrides;
use std::ffi::OsString;

fn owned(value: &str) -> OsString {
    OsString::from(value)
}

/// The seeds the binary would produce for `input`, as one EnvSeed layer.
fn seed_layer(input: NuwaxEnvInput) -> ConfigLayerEntry {
    let seeds = nuwax_env_overrides(
        input,
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect("the group seeds");
    let mut table = toml::Table::new();
    for (key, value) in seeds {
        // The loader reads dotted keys as nested tables; mirror that so the
        // reserved provider's table is a table, as the validator expects.
        let parts: Vec<&str> = key.split('.').collect();
        let mut cursor = &mut table;
        for (index, part) in parts.iter().enumerate() {
            let entry = cursor
                .entry((*part).to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if index + 1 == parts.len() {
                *entry = value;
                break;
            }
            cursor = entry.as_table_mut().expect("a nested table");
        }
    }
    ConfigLayerEntry::new(ConfigLayerSource::EnvSeed, toml::Value::Table(table))
}

fn stack(layers: Vec<ConfigLayerEntry>) -> ConfigLayerStack {
    ConfigLayerStack::new(
        layers,
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("layer stack")
}

#[test]
fn the_groups_seeds_pass_the_reserved_provider_isolation_check() {
    let input = NuwaxEnvInput {
        model: Some(owned("env-model")),
        base_url: Some(owned("https://gateway.example/v1")),
        wire_api: Some(owned("chat")),
        api_key: Some(owned("smoke-key")),
        max_output_tokens: Some(owned("4096")),
        request_max_retries: Some(owned("1")),
        stream_max_retries: Some(owned("1")),
        stream_idle_timeout_ms: Some(owned("120000")),
        bridge: Some(owned("rig")),
        reasoning_effort_levels: Some(owned("low,high,max")),
    };
    if let Err(error) =
        validate_env_group_isolation(&stack(vec![seed_layer(input)]), NUWAX_ENV_PROVIDER_ID)
    {
        panic!("every key the group seeds must be whitelisted: {error}");
    }
}

/// The check is what makes the group's table safe to select, so a group that
/// seeds nothing must still fail — otherwise this test would pass vacuously.
#[test]
fn an_empty_seed_layer_still_fails_the_check() {
    let error = validate_env_group_isolation(
        &stack(vec![ConfigLayerEntry::new(
            ConfigLayerSource::EnvSeed,
            toml::from_str("model = \"env-model\"").expect("seed toml"),
        )]),
        NUWAX_ENV_PROVIDER_ID,
    )
    .expect_err("no provider table means the group is not active");
    assert!(error.to_string().contains("not active"), "{}", error);
}
