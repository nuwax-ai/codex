//! Fork (nuwax-codex): load-time source isolation for the `NUWAX_*`
//! temporary provider.
//!
//! The environment startup group seeds `model_providers.nuwax_env` through a
//! dedicated [`ConfigLayerSource::EnvSeed`] layer, so the contribution keeps
//! its provenance through merging. A key-name whitelist over the MERGED
//! table cannot prove origin: a config file, profile, managed requirement,
//! `-c` override or thread-config layer could contribute a same-named
//! `base_url` or `env_key` and silently redirect the environment credential
//! to another endpoint. Loading therefore fails fast when the reserved
//! provider is selected and ANY layer other than the seed layer contributes
//! to its table — errors name the layer source and key names, never values.

use crate::ConfigLayerStack;
use crate::config_layer_source::ConfigLayerSource;
use crate::config_layer_source::format_config_layer_source;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use std::io;
use toml::Value as TomlValue;

/// The only keys the environment group itself seeds; a defensive backstop on
/// the merged table — the authoritative check is per-layer provenance.
const ENV_GROUP_SEED_KEYS: [&str; 8] = [
    "name",
    "base_url",
    "wire_api",
    "env_key",
    "max_output_tokens",
    "request_max_retries",
    "stream_max_retries",
    "stream_idle_timeout_ms",
];

/// Validates the reserved provider's origin using the caller's final
/// selection, after requirements and typed overrides are resolved. When the
/// reserved provider is selected, only the environment seed layer may define
/// its table, and the seed layer must exist.
pub fn validate_env_group_isolation(
    stack: &ConfigLayerStack,
    selected_provider: &str,
) -> io::Result<()> {
    if selected_provider != NUWAX_ENV_PROVIDER_ID {
        return Ok(());
    }
    let reserved = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "model_providers.{NUWAX_ENV_PROVIDER_ID} is reserved for the \
                 NUWAX_* environment provider"
            ),
        )
    };
    let seed_table = stack
        .layers_low_to_high()
        .filter(|layer| matches!(layer.name, ConfigLayerSource::EnvSeed))
        .find_map(|layer| reserved_table(&layer.config));
    if seed_table.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "model_providers.{NUWAX_ENV_PROVIDER_ID} is reserved for the \
                 NUWAX_* environment provider, but the environment group is \
                 not active in this process; unset the provider selection or \
                 set NUWAX_BASE_URL, NUWAX_WIRE_API and NUWAX_API_KEY together"
            ),
        ));
    }
    for layer in stack.layers_low_to_high() {
        if matches!(layer.name, ConfigLayerSource::EnvSeed) {
            continue;
        }
        let Some(table) = reserved_table(&layer.config) else {
            continue;
        };
        let mut keys = table_keys(table);
        keys.sort_unstable();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} sets [{}] in model_providers.{NUWAX_ENV_PROVIDER_ID}; \
                 only the NUWAX_* environment group may define it (remove \
                 those fields or unset the NUWAX_* environment group)",
                format_config_layer_source(&layer.name, crate::CONFIG_TOML_FILE),
                keys.join(", "),
            ),
        ));
    }
    if stack
        .requirements_toml()
        .model_providers
        .as_ref()
        .is_some_and(|providers| providers.contains_key(NUWAX_ENV_PROVIDER_ID))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "managed requirements define model_providers.\
                 {NUWAX_ENV_PROVIDER_ID}; only the NUWAX_* environment group \
                 may define it"
            ),
        ));
    }
    // Defensive backstop on the merged table: the seed itself must not have
    // contributed anything beyond the group's keys.
    if let Some(table) = stack
        .effective_config()
        .get("model_providers")
        .and_then(|providers| providers.get(NUWAX_ENV_PROVIDER_ID))
        .and_then(TomlValue::as_table)
    {
        let mut foreign: Vec<&str> = table
            .keys()
            .filter(|key| !ENV_GROUP_SEED_KEYS.contains(&key.as_str()))
            .map(String::as_str)
            .collect();
        if !foreign.is_empty() {
            foreign.sort_unstable();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}; the effective table also sets [{}]",
                    reserved(),
                    foreign.join(", "),
                ),
            ));
        }
    }
    Ok(())
}

/// The reserved provider's table as defined by this layer's own config, if
/// the layer contributes anything to it.
fn reserved_table(config: &TomlValue) -> Option<&TomlValue> {
    config.get("model_providers")?.get(NUWAX_ENV_PROVIDER_ID)
}

fn table_keys(table: &TomlValue) -> Vec<&str> {
    table
        .as_table()
        .map(|table| table.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "env_group_isolation_tests.rs"]
mod tests;
