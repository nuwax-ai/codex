//! Vendor configuration and environment-file precedence for live and offline runs.

use std::collections::HashMap;
use std::path::Path;

use crate::CassetteMode;
use crate::cassette_mode;
use crate::repo_root;

pub struct LiveConfig {
    /// Vendor tag: names the provider in generated config and the artifact
    /// directory (`logs/live-<vendor>/`). Defaults to `mimo`.
    pub vendor: String,
    pub api_key: String,
    pub base_url: String,
    /// Anthropic-protocol gateway, when this vendor exposes one. Anthropic
    /// suites skip when `None` — there must never be a cross-vendor default
    /// here (sending vendor A's key to vendor B's endpoint).
    pub anthropic_base_url: Option<String>,
    /// Responses-API endpoint, when it differs from the chat endpoint
    /// (GLM: chat at `/api/coding/paas/v4`, responses at `/api/v1`).
    /// `None` defaults to the chat URL (same-origin deployments like MiMo).
    pub responses_base_url: Option<String>,
    pub model: String,
}

/// Enumerates every vendor under test.
///
/// Primary mode — matrix: `LIVE_VENDORS=mimo,glm` in `.env.local`, with
/// per-vendor variables `LIVE_<NAME>_API_KEY` / `LIVE_<NAME>_CHAT_URL` /
/// `LIVE_<NAME>_ANTHROPIC_URL` (optional) / `LIVE_<NAME>_MODEL`. Tests are
/// generated per vendor (see the `vendor_matrix!` macros in the suites), so
/// each vendor reports its own pass/fail granularity.
///
/// Fallback mode — single vendor: when `LIVE_VENDORS` is unset, the legacy
/// `LIVE_VENDOR_*` (and `MIMO_*`) variables configure exactly one vendor,
/// defaulting to `mimo`.
///
/// URL rules: no cross-vendor fallbacks ever. The `mimo` vendor has built-in
/// defaults; every other vendor must declare its URLs explicitly (missing
/// declarations skip that vendor with a notice instead of guessing).
pub fn vendors() -> Vec<LiveConfig> {
    let file_env = load_env_files();
    let lookup = |key: &str| -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| file_env.get(key).cloned())
    };
    let names: Vec<String> = match lookup("LIVE_VENDORS") {
        Some(list) => list
            .split(',')
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty())
            .collect(),
        None => vec![lookup("LIVE_VENDOR_NAME").unwrap_or_else(|| "mimo".into())],
    };
    names
        .iter()
        .filter_map(|name| vendor_from_env(&lookup, name, cassette_mode()))
        .collect()
}

/// The configuration for one named vendor, when it is configured.
pub fn vendor(name: &str) -> Option<LiveConfig> {
    vendors().into_iter().find(|v| v.vendor == name)
}

fn vendor_from_env(
    lookup: &dyn Fn(&str) -> Option<String>,
    name: &str,
    mode: CassetteMode,
) -> Option<LiveConfig> {
    let upper = name.to_uppercase().replace('-', "_");
    let (key_var, chat_var, anthropic_var, model_var) = if lookup("LIVE_VENDORS").is_some() {
        (
            format!("LIVE_{upper}_API_KEY"),
            format!("LIVE_{upper}_CHAT_URL"),
            format!("LIVE_{upper}_ANTHROPIC_URL"),
            format!("LIVE_{upper}_MODEL"),
        )
    } else {
        // Legacy single-vendor variables (mimo keeps its built-in defaults).
        (
            "LIVE_VENDOR_API_KEY".to_string(),
            "LIVE_VENDOR_CHAT_URL".to_string(),
            "LIVE_VENDOR_ANTHROPIC_URL".to_string(),
            "LIVE_VENDOR_MODEL".to_string(),
        )
    };
    let is_mimo = name == "mimo";
    // MIMO_* stays as a fallback so the original .env.local keeps working.
    let api_key = lookup(&key_var).or_else(|| is_mimo.then(|| lookup("MIMO_API_KEY")).flatten());
    let api_key = match api_key {
        Some(key) => key,
        // Replay mode is fully offline: cassette fixtures carry everything
        // the assertions need, so a placeholder key keeps the vendor active.
        None if mode == CassetteMode::Replay => "(replay-placeholder)".to_string(),
        None => {
            println!("no API key configured for vendor `{name}` ({key_var}) — skipping vendor");
            return None;
        }
    };
    let base_url = lookup(&chat_var).or_else(|| is_mimo.then(|| lookup("MIMO_BASE_URL")).flatten());
    let base_url = match base_url {
        Some(url) => url,
        // Replay mode is fully offline: fixtures carry everything the
        // conversion needs, so placeholder URLs/models keep the vendor
        // active for bridge-boundary replay.
        None if mode == CassetteMode::Replay => "(replay-placeholder-url)".to_string(),
        None => {
            println!("no chat URL configured for vendor `{name}` ({chat_var}) — skipping vendor");
            return None;
        }
    };
    let model = lookup(&model_var).or_else(|| is_mimo.then(|| lookup("MIMO_MODEL")).flatten());
    let model = match model {
        Some(m) => m,
        None if mode == CassetteMode::Replay => "(replay-placeholder-model)".to_string(),
        None => {
            println!("no model configured for vendor `{name}` ({model_var}) — skipping vendor");
            return None;
        }
    };
    let anthropic_base_url = lookup(&anthropic_var)
        .or_else(|| is_mimo.then(|| lookup("MIMO_ANTHROPIC_BASE_URL")).flatten())
        .or_else(|| {
            (mode == CassetteMode::Replay).then(|| "(replay-placeholder-anthropic-url)".into())
        });
    let responses_var = if lookup("LIVE_VENDORS").is_some() {
        format!("LIVE_{upper}_RESPONSES_URL")
    } else {
        "LIVE_VENDOR_RESPONSES_URL".to_string()
    };
    let responses_base_url = lookup(&responses_var).or(Some(base_url.clone()));
    Some(LiveConfig {
        vendor: name.to_string(),
        api_key,
        base_url,
        anthropic_base_url,
        responses_base_url,
        model,
    })
}

/// Parses `.env` then `.env.local` (higher priority) from the repository
/// root into a local map. Nothing is written to the process environment
/// (that would require `unsafe` on edition 2024).
pub fn load_env_files() -> HashMap<String, String> {
    repo_root()
        .map(|root| load_env_files_from(&root))
        .unwrap_or_default()
}

fn load_env_files_from(root: &Path) -> HashMap<String, String> {
    let mut merged = HashMap::new();
    for file in [".env", ".env.local"] {
        let Ok(contents) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let mut value = value.trim().to_string();
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = value[1..value.len() - 1].to_string();
            }
            merged.insert(key.trim().to_string(), value);
        }
    }
    merged
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
