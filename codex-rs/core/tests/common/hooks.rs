use codex_config::CONFIG_TOML_FILE;
use codex_config::ConfigLayerStack;
use codex_config::TomlValue;
use codex_core::config::Config;
use codex_features::Feature;
use codex_hooks::HookListEntry;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::fs;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tokio::time::timeout;

/// Writes one logging command hook per listed event into `hooks.json`; each
/// invocation appends its stdin (the hook input JSON) to `<event>.jsonl`.
pub fn write_lifecycle_hooks(home: &Path, events: &[&str]) -> Result<()> {
    let mut hooks = serde_json::Map::new();
    for event in events {
        let log = home.join(format!("{event}.jsonl"));
        let command = format!(
            "cat >> '{log}'; echo >> '{log}'; printf '%s' '{{\"continue\":false,\"decision\":\"block\",\"reason\":\"ignored-observer-control\"}}'",
            log = log.display(),
        );
        hooks.insert(
            (*event).into(),
            json!([{"hooks": [{"type": "command", "command": command}]}]),
        );
    }
    fs::write(home.join("hooks.json"), json!({"hooks": hooks}).to_string())?;
    Ok(())
}

pub async fn wait_for_hook_inputs(home: &Path, event: &str, count: usize) -> Result<Vec<Value>> {
    timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(inputs) = read_hook_inputs_from_log(&home.join(format!("{event}.jsonl")))
                && inputs.len() >= count
            {
                return Ok(inputs);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

pub fn read_hook_inputs_from_log(log_path: &Path) -> Result<Vec<Value>> {
    fs::read_to_string(log_path)
        .with_context(|| format!("read hook log {}", log_path.display()))?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("parse hook log line"))
        .collect()
}

pub fn trust_discovered_hooks(config: &mut Config) {
    config
        .features
        .enable(Feature::CodexHooks)
        .expect("test config should allow feature update");

    let listed = codex_hooks::list_hooks(codex_hooks::HooksConfig {
        feature_enabled: true,
        config_layer_stack: Some(config.config_layer_stack.clone()),
        ..codex_hooks::HooksConfig::default()
    });
    assert!(
        !listed.hooks.is_empty(),
        "trusted hook fixture should discover at least one hook"
    );
    trust_hooks(config, listed.hooks);
}

pub fn trust_hooks(config: &mut Config, hooks: Vec<HookListEntry>) {
    config.config_layer_stack =
        trusted_config_layer_stack(&config.config_layer_stack, &config.codex_home, hooks);
}

pub fn trusted_config_layer_stack(
    config_layer_stack: &ConfigLayerStack,
    codex_home: &AbsolutePathBuf,
    hooks: Vec<HookListEntry>,
) -> ConfigLayerStack {
    let mut user_config = config_layer_stack
        .get_active_user_layer()
        .map(|layer| layer.config.clone())
        .unwrap_or_else(|| TomlValue::Table(Default::default()));
    let user_table = user_config
        .as_table_mut()
        .expect("user config should be a table");
    let hooks_table = user_table
        .entry("hooks")
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .expect("hooks config should be a table");
    let state_table = hooks_table
        .entry("state")
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .expect("hook state config should be a table");
    for hook in hooks {
        let mut hook_state = TomlValue::Table(Default::default());
        let hook_state_table = hook_state
            .as_table_mut()
            .expect("hook state should be a table");
        hook_state_table.insert(
            "trusted_hash".to_string(),
            TomlValue::String(hook.current_hash),
        );
        state_table.insert(hook.key, hook_state);
    }

    config_layer_stack
        .with_user_config(&codex_home.join(CONFIG_TOML_FILE), user_config)
        .expect("hook user config should be valid")
}
