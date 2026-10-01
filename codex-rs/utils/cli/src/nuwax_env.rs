//! Fork (nuwax-codex): the `NUWAX_*` process-environment startup group.
//!
//! `NUWAX_BASE_URL` + `NUWAX_WIRE_API` + `NUWAX_API_KEY` (with
//! `NUWAX_MODEL`) start a session against a temporary custom provider for
//! this one process, without touching config files. The group is parsed and
//! validated ONCE per process into seed overrides for the existing `-c`
//! pipeline — no parallel provider storage. Credentials are only ever
//! referenced (`env_key = "NUWAX_API_KEY"`); the raw key never enters a
//! config value, rollout, or diagnostic.
//!
//! Precedence falls out of the existing layers: an explicit typed CLI model
//! (`-m`) or provider selection beats these seeds, and an explicit `-c` key
//! beats the environment. Selecting a different provider explicitly makes
//! the whole group irrelevant, so it is neither applied nor validated in
//! that case — an unused, malformed env value must not break a valid CLI
//! configuration.

use std::ffi::OsString;

use toml::Table;
use toml::Value;

/// Reserved provider id for the per-run provider (§ spec 5.1): a collision
/// with a user-configured provider of the same id is a hard error, never a
/// silent merge of auth fields. Canonical definition in
/// `codex_protocol::config_types` so the config loader enforces the same
/// reservation at effective-config time.
pub use codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID;

const MODEL_ENV: &str = "NUWAX_MODEL";
const BASE_URL_ENV: &str = "NUWAX_BASE_URL";
const WIRE_API_ENV: &str = "NUWAX_WIRE_API";
const API_KEY_ENV: &str = "NUWAX_API_KEY";

/// The raw environment inputs, owned so the production reader can move the
/// `var_os` values. Non-Unicode values surface as a diagnosable error
/// instead of a lossy conversion. Tests construct this directly;
/// [`from_process`] is the single production reader.
#[derive(Debug, Default, Clone)]
pub struct NuwaxEnvInput {
    pub model: Option<OsString>,
    pub base_url: Option<OsString>,
    pub wire_api: Option<OsString>,
    pub api_key: Option<OsString>,
}

/// Reads the group from the real process environment exactly once per call
/// site. Callers invoke this once during startup; nothing re-reads the
/// environment afterwards.
pub fn from_process() -> NuwaxEnvInput {
    NuwaxEnvInput {
        model: std::env::var_os(MODEL_ENV),
        base_url: std::env::var_os(BASE_URL_ENV),
        wire_api: std::env::var_os(WIRE_API_ENV),
        api_key: std::env::var_os(API_KEY_ENV),
    }
}

/// Computes the seed overrides for the startup group.
///
/// * `cli_model` / `cli_provider` — the typed CLI selections (`-m`, `--oss`
///   provider). An explicit provider selection makes the group irrelevant:
///   the result is empty and the env values are not validated.
/// * `existing` — the already-parsed explicit `-c` keys; any key it already
///   set wins over the environment.
///
/// Errors name the offending variable and the expectation, never values
/// (the API key in particular).
pub fn nuwax_env_overrides(
    input: NuwaxEnvInput,
    cli_model: Option<&str>,
    cli_provider: Option<&str>,
    existing: &[(String, Value)],
) -> Result<Vec<(String, Value)>, String> {
    if cli_provider.is_some() {
        // The group is unused; §5.2 forbids failing a valid CLI config on
        // unadopted env values.
        return Ok(Vec::new());
    }
    let has_existing = |key: &str| existing.iter().any(|(other, _)| other == key);
    if has_existing("model_provider") {
        // An explicit -c provider selection is treated like a typed one.
        // Selecting the reserved id uses the full group (below); anything
        // else makes it irrelevant.
        let selected = existing
            .iter()
            .rev()
            .find(|(key, _)| key == "model_provider")
            .and_then(|(_, value)| value.as_str())
            .unwrap_or_default();
        if selected != NUWAX_ENV_PROVIDER_ID {
            return Ok(Vec::new());
        }
    }
    // Do not validate an environment model that an explicit selection replaces.
    let model = if cli_model.is_some() || has_existing("model") {
        None
    } else {
        unicode(input.model.as_deref(), MODEL_ENV)?
    };
    let base_url = unicode(input.base_url.as_deref(), BASE_URL_ENV)?;
    let wire_api = unicode(input.wire_api.as_deref(), WIRE_API_ENV)?;
    let api_key = unicode(input.api_key.as_deref(), API_KEY_ENV)?;

    let group_set = [base_url, wire_api, api_key];
    let set_count = group_set.iter().filter(|value| value.is_some()).count();
    if set_count == 0 {
        // Group inactive: NUWAX_MODEL may still select an existing
        // provider's model, at env precedence (below explicit -c and the
        // typed CLI flag).
        return seed_model_only(model);
    }
    if set_count < 3 {
        let missing = [
            (BASE_URL_ENV, base_url.is_none()),
            (WIRE_API_ENV, wire_api.is_none()),
            (API_KEY_ENV, api_key.is_none()),
        ]
        .into_iter()
        .filter(|(_, missing)| *missing)
        .map(|(name, _)| name)
        .collect::<Vec<_>>()
        .join(", ");
        return Err(format!(
            "NUWAX temporary provider group is partially set (missing {missing}); \
             set NUWAX_BASE_URL, NUWAX_WIRE_API and NUWAX_API_KEY together, or unset them all"
        ));
    }
    // Group active: validate every adopted field. Errors carry field names
    // and expectations only.
    let base_url = require_non_blank(base_url, BASE_URL_ENV)?;
    validate_base_url(base_url)?;
    let wire_api = require_non_blank(wire_api, WIRE_API_ENV)?;
    if !matches!(wire_api, "responses" | "chat" | "anthropic") {
        return Err(format!(
            "Invalid {WIRE_API_ENV} value; expected one of responses, chat or anthropic"
        ));
    }
    require_non_blank(api_key, API_KEY_ENV)?;
    if has_existing(&format!("model_providers.{NUWAX_ENV_PROVIDER_ID}")) {
        return Err(format!(
            "model_providers.{NUWAX_ENV_PROVIDER_ID} is a reserved id for the NUWAX \
             environment provider; remove the configured provider or unset the \
             NUWAX_* environment group"
        ));
    }
    let model = match model {
        Some(model) => Some(require_non_blank(Some(model), MODEL_ENV)?),
        None => None,
    };

    let mut seeds = Vec::new();
    let mut provider = Table::new();
    provider.insert("name".into(), Value::String("nuwax env provider".into()));
    provider.insert("base_url".into(), Value::String(base_url.to_string()));
    provider.insert("wire_api".into(), Value::String(wire_api.to_string()));
    // Credential by reference: the provider pipeline resolves (and fails
    // fast on) a missing env var at auth time; the key itself is never
    // copied into configuration.
    provider.insert("env_key".into(), Value::String(API_KEY_ENV.into()));
    seeds.push((
        format!("model_providers.{NUWAX_ENV_PROVIDER_ID}"),
        Value::Table(provider),
    ));
    if !has_existing("model_provider") {
        seeds.push((
            "model_provider".into(),
            Value::String(NUWAX_ENV_PROVIDER_ID.into()),
        ));
    }
    // The group needs an explicit model (§5.1.1). The typed CLI flag and an
    // explicit -c both outrank the environment, so no model seed is needed
    // (and none may be added) when either already provides one.
    if !has_existing("model") && cli_model.is_none() {
        let model = model.ok_or_else(|| {
            format!(
                "NUWAX temporary provider requires a model; set {MODEL_ENV} or pass an \
                 explicit --model"
            )
        })?;
        seeds.push(("model".into(), Value::String(model.to_string())));
    }
    Ok(seeds)
}

fn seed_model_only(model: Option<&str>) -> Result<Vec<(String, Value)>, String> {
    let Some(model) = model else {
        return Ok(Vec::new());
    };
    let model = require_non_blank(Some(model), MODEL_ENV)?;
    Ok(vec![("model".into(), Value::String(model.to_string()))])
}

/// Converts one variable to `&str`, mapping non-Unicode to an error that
/// names the variable (never the value).
fn unicode<'a>(value: Option<&'a std::ffi::OsStr>, name: &str) -> Result<Option<&'a str>, String> {
    value
        .map(|value| {
            value
                .to_str()
                .ok_or_else(|| format!("Invalid {name}: expected Unicode text"))
        })
        .transpose()
}

/// Blank values are indistinguishable from unset intent; reject them loudly.
fn require_non_blank<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, String> {
    let value = value.unwrap_or_default().trim();
    if value.is_empty() {
        return Err(format!(
            "Invalid {name} value: expected non-blank text (the variable is set)"
        ));
    }
    Ok(value)
}

/// Absolute HTTP/HTTPS URL with a host and no credentials; anything else
/// fails the group before a request is ever built. Parsed with the standard
/// URL type so hostless authorities, invalid ports and malformed IPv6 are
/// rejected by the parser, and credentials in the userinfo never reach an
/// endpoint. No URL-based protocol inference — the wire comes from
/// `NUWAX_WIRE_API` alone.
fn validate_base_url(base_url: &str) -> Result<(), String> {
    let invalid = || {
        format!(
            "Invalid {BASE_URL_ENV} value: expected an absolute http:// or https:// URL \
             with a host and without credentials"
        )
    };
    let url = url::Url::parse(base_url).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(invalid());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "nuwax_env_tests.rs"]
mod tests;
