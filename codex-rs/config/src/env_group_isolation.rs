//! Fork (nuwax-codex): load-time isolation for the `NUWAX_*` temporary
//! provider.
//!
//! The environment startup group seeds `model_providers.nuwax_env` with
//! exactly four keys (name, base_url, wire_api, env_key) and selects it.
//! Config layers merge recursively, so a same-named provider defined in a
//! config file, profile, project config or `-c` override contributes its
//! fields to the merged table — stale credentials above all — and those
//! ride the requests sent to the environment endpoint. Loading fails fast
//! instead: foreign keys in the selected reserved provider are a hard error
//! naming the keys (never their values).

use crate::ConfigLayerStack;
use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;
use std::io;
use toml::Value as TomlValue;

/// The only keys the environment group itself seeds; anything else in the
/// effective reserved-provider table came from another configuration layer.
const ENV_GROUP_SEED_KEYS: [&str; 4] = ["name", "base_url", "wire_api", "env_key"];

/// Validates the merged layer stack: when the reserved environment provider
/// is the selected provider, its effective table may only contain the keys
/// the group seeds. Covers the initial load and every runtime refresh —
/// both assemble their `ConfigToml` through this crate's layer stack.
pub fn validate_env_group_isolation(stack: &ConfigLayerStack) -> io::Result<()> {
    let merged = stack.effective_config();
    let selected = merged
        .get("model_provider")
        .and_then(TomlValue::as_str)
        .or(stack.required_model_provider());
    if selected != Some(NUWAX_ENV_PROVIDER_ID) {
        return Ok(());
    }
    let Some(table) = merged
        .get("model_providers")
        .and_then(|providers| providers.get(NUWAX_ENV_PROVIDER_ID))
        .and_then(TomlValue::as_table)
    else {
        // A missing or non-table entry fails provider resolution downstream
        // with its own diagnostics; there is nothing to isolate here.
        return Ok(());
    };
    let mut foreign: Vec<&str> = table
        .keys()
        .filter(|key| !ENV_GROUP_SEED_KEYS.contains(&key.as_str()))
        .map(String::as_str)
        .collect();
    if foreign.is_empty() {
        return Ok(());
    }
    foreign.sort_unstable();
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "model_providers.{NUWAX_ENV_PROVIDER_ID} is reserved for the NUWAX_* \
             environment provider, but the effective configuration also sets \
             [{}] there; remove those fields or unset the NUWAX_* environment \
             group",
            foreign.join(", "),
        ),
    ))
}

#[cfg(test)]
#[path = "env_group_isolation_tests.rs"]
mod tests;
