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
        let command = lifecycle_hook_command(&log);
        hooks.insert(
            (*event).into(),
            json!([{"hooks": [{"type": "command", "command": command}]}]),
        );
    }
    fs::write(home.join("hooks.json"), json!({"hooks": hooks}).to_string())?;
    Ok(())
}

const OBSERVER_CONTROL: &str =
    r#"{"continue":false,"decision":"block","reason":"ignored-observer-control"}"#;

#[cfg(windows)]
fn lifecycle_hook_command(log: &Path) -> String {
    use base64::Engine;

    // The hook runtime uses COMSPEC on Windows. Encode the PowerShell source
    // so cmd.exe does not reinterpret its syntax or Unicode paths.
    let log = log.display().to_string().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference = 'Stop'; [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); $input_json = [Console]::In.ReadToEnd(); [System.IO.File]::AppendAllText('{log}', $input_json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false)); [Console]::Out.Write('{OBSERVER_CONTROL}');"
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!("powershell.exe -NoProfile -NonInteractive -EncodedCommand {encoded}")
}

#[cfg(not(windows))]
fn lifecycle_hook_command(log: &Path) -> String {
    let log = log.display().to_string().replace('\'', "'\\''");
    format!("cat >> '{log}'; echo >> '{log}'; printf '%s' '{OBSERVER_CONTROL}'")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn lifecycle_hook_shell_preserves_utf8_jsonl_and_observer_control() -> Result<()> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("space 中文 O'Brien");
        fs::create_dir(&home)?;
        write_lifecycle_hooks(&home, &["PostModel"])?;
        let config: Value = serde_json::from_slice(&fs::read(home.join("hooks.json"))?)?;
        let command = config["hooks"]["PostModel"][0]["hooks"][0]["command"]
            .as_str()
            .expect("logging hook command");
        let inputs = vec![
            json!({
                "hook_event_name": "PostModel",
                "cwd": home.display().to_string(),
                "prompt": "中文, apostrophe ', double quote \", newline\nnext line",
                "nested": {"ok": true, "tokens": 7, "items": [null, "工具"]},
            }),
            json!({"hook_event_name": "PostModel", "input": "第二行", "attempt": 2}),
        ];
        for input in &inputs {
            #[cfg(windows)]
            let mut process = {
                use std::os::windows::process::CommandExt;

                let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
                let mut process = tokio::process::Command::new(shell);
                process.arg("/C");
                process.as_std_mut().raw_arg(format!(r#""{command}""#));
                process
            };
            #[cfg(not(windows))]
            let mut process = {
                let mut process = tokio::process::Command::new("/bin/sh");
                process.arg("-lc").arg(command);
                process
            };
            let mut child = process
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            let output = timeout(Duration::from_secs(10), async {
                let mut stdin = child.stdin.take().expect("piped hook stdin");
                stdin.write_all(&serde_json::to_vec(input)?).await?;
                drop(stdin);
                child.wait_with_output().await.map_err(anyhow::Error::from)
            })
            .await??;
            assert!(
                output.status.success(),
                "logging hook failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout)?,
                json!({
                    "continue": false,
                    "decision": "block",
                    "reason": "ignored-observer-control",
                })
            );
        }
        let log = home.join("PostModel.jsonl");
        assert!(!fs::read(&log)?.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(read_hook_inputs_from_log(&log)?, inputs);
        Ok(())
    }
}
