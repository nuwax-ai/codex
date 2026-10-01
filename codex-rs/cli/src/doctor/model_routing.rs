//! Model routing diagnostics: the effective model, provider, wire, bridge,
//! redacted endpoint and compaction threshold — enough to see, from a
//! `doctor --json` report, exactly where a session would send requests.

use super::CheckStatus;
use super::DoctorCheck;
use codex_core::build_models_manager;
use codex_core::config::Config;
use codex_login::AuthManager;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use std::sync::Arc;

pub(super) async fn check(config: &Config, auth_manager: Option<Arc<AuthManager>>) -> DoctorCheck {
    let mut details = Vec::new();
    let provider = &config.model_provider;
    details.push(format!(
        "model: {}",
        config.model.as_deref().unwrap_or("<default>")
    ));
    details.push(format!("provider id: {}", config.model_provider_id));
    details.push(format!("wire api: {}", provider.wire_api));
    let bridged = provider.uses_model_bridge();
    details.push(format!(
        "bridge: {}",
        if bridged {
            "rig (model bridge)"
        } else {
            "native transport"
        }
    ));
    details.push(format!(
        "endpoint (redacted): {}",
        redacted_endpoint(provider.base_url.as_deref())
    ));
    if let Some(experimental) = provider.experimental_bridge {
        details.push(format!("experimental bridge override: {experimental:?}"));
    }
    // Compaction: report both the configured inputs and the derived
    // threshold against the effective model window (where one is knowable
    // without a catalog fetch failure blocking the whole check).
    details.push(format!(
        "model context window override: {}",
        config
            .model_context_window
            .map(|window| window.to_string())
            .unwrap_or_else(|| "<model default>".into())
    ));
    details.push(format!(
        "auto compact absolute limit: {}",
        config
            .model_auto_compact_token_limit
            .map(|limit| limit.to_string())
            .unwrap_or_else(|| "<unset>".into())
    ));
    details.push(format!(
        "auto compact ratio: {}",
        config
            .model_auto_compact_ratio
            .map(|ratio| ratio.to_string())
            .unwrap_or_else(|| "<unset>".into())
    ));
    details.push(format!(
        "auto compact limit scope: {}",
        config.model_auto_compact_token_limit_scope
    ));
    let backend = if config.model_provider_id == "nuwax_env" {
        "embedded app-server (NUWAX environment provider: per-run credentials \
         cannot ride a shared daemon)"
    } else {
        "default daemon policy (see config.load and terminal checks)"
    };
    details.push(format!("app-server backend: {backend}"));

    let mut status = CheckStatus::Ok;
    let mut summary = "model routing resolved".to_string();
    match auth_manager {
        Some(auth_manager) => {
            let manager = build_models_manager(config, auth_manager);
            let model = config.model.clone().unwrap_or_default();
            let overrides = config.to_models_manager_config();
            let model_info = manager.get_model_info(&model, &overrides).await;
            let compact_limit = match config.model_auto_compact_token_limit_scope {
                AutoCompactTokenLimitScope::Total => model_info.auto_compact_token_limit(),
                AutoCompactTokenLimitScope::BodyAfterPrefix => config
                    .model_auto_compact_token_limit
                    .or_else(|| model_info.auto_compact_token_limit()),
            };
            details.push(format!(
                "effective context window: {}",
                model_info
                    .resolved_context_window()
                    .map(|window| window.to_string())
                    .unwrap_or_else(|| "<unknown>".into())
            ));
            details.push(format!(
                "full usable context window: {}",
                model_info
                    .usable_context_window()
                    .map(|window| window.to_string())
                    .unwrap_or_else(|| "<unknown>".into())
            ));
            details.push(format!(
                "effective auto compact threshold: {}",
                compact_limit
                    .map(|limit| limit.to_string())
                    .unwrap_or_else(|| "<model default>".into())
            ));
        }
        None => {
            status = CheckStatus::Warning;
            summary = "model routing resolved without auth; threshold derivation skipped".into();
        }
    }

    DoctorCheck::new("config.model_routing", "config", status, summary).details(details)
}

/// Endpoint reduced to scheme://host[:port] — paths and query strings can
/// carry credentials (gateway tokens in query params) and add no routing
/// information.
fn redacted_endpoint(base_url: Option<&str>) -> String {
    let Some(base_url) = base_url else {
        return "<provider default>".into();
    };
    let Ok(url) = url::Url::parse(base_url) else {
        return "<unparsed>".into();
    };
    let Some(host) = url.host() else {
        return "<unparsed>".into();
    };
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    format!("{}://{host}{port}", url.scheme())
}

#[cfg(test)]
#[path = "model_routing_tests.rs"]
mod tests;
