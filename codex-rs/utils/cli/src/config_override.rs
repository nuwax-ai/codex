//! Support for `-c key=value` overrides shared across Codex CLI tools.
//!
//! This module provides a [`CliConfigOverrides`] struct that can be embedded
//! into a `clap`-derived CLI struct using `#[clap(flatten)]`. Each occurrence
//! of `-c key=value` (or `--config key=value`) will be collected as a raw
//! string. Helper methods are provided to convert the raw strings into
//! key/value pairs as well as to apply them onto a mutable
//! `serde_json::Value` representing the configuration tree.

use clap::ArgAction;
use clap::Parser;
use serde::de::Error as SerdeError;
use std::ffi::OsStr;
use toml::Value;

/// CLI option that captures arbitrary configuration overrides specified as
/// `-c key=value`. It intentionally keeps both halves **unparsed** so that the
/// calling code can decide how to interpret the right-hand side.
#[derive(Parser, Debug, Default, Clone)]
pub struct CliConfigOverrides {
    /// Override a configuration value that would otherwise be loaded from
    /// `~/.codex/config.toml`. Use a dotted path (`foo.bar.baz`) to override
    /// nested values. The `value` portion is parsed as TOML. If it fails to
    /// parse as TOML, the raw string is used as a literal.
    ///
    /// Examples:
    ///   - `-c model="o3"`
    ///   - `-c 'sandbox_permissions=["disk-full-read-access"]'`
    ///   - `-c shell_environment_policy.inherit=all`
    #[arg(
        short = 'c',
        long = "config",
        value_name = "key=value",
        action = ArgAction::Append,
        global = true,
    )]
    pub raw_overrides: Vec<String>,
}

impl CliConfigOverrides {
    /// Prepend root-level config flags so they have lower precedence than
    /// command-specific flags parsed after a subcommand.
    pub fn prepend_root_overrides(&mut self, root_overrides: Self) {
        self.raw_overrides
            .splice(0..0, root_overrides.raw_overrides);
    }

    /// Parse the raw strings captured from the CLI into a list of `(path,
    /// value)` tuples where `value` is a `serde_json::Value`.
    pub fn parse_overrides(&self) -> Result<Vec<(String, Value)>, String> {
        let mut overrides: Vec<(String, Value)> = self
            .raw_overrides
            .iter()
            .map(|s| {
                // Only split on the *first* '=' so values are free to contain
                // the character.
                let mut parts = s.splitn(2, '=');
                let key = match parts.next() {
                    Some(k) => k.trim(),
                    None => return Err("Override missing key".to_string()),
                };
                let value_str = parts
                    .next()
                    .ok_or_else(|| format!("Invalid override (missing '='): {s}"))?
                    .trim();

                if key.is_empty() {
                    return Err(format!("Empty key in override: {s}"));
                }

                // Attempt to parse as TOML. If that fails, treat it as a raw
                // string. This allows convenient usage such as
                // `-c model=o3` without the quotes.
                let value: Value = match parse_toml_value(value_str) {
                    Ok(v) => v,
                    Err(_) => {
                        // Strip leading/trailing quotes if present
                        let trimmed = value_str.trim().trim_matches(|c| c == '"' || c == '\'');
                        Value::String(trimmed.to_string())
                    }
                };

                Ok((canonicalize_override_key(key), value))
            })
            .collect::<Result<Vec<_>, String>>()?;
        apply_env_effort_override(
            &mut overrides,
            std::env::var_os(MODEL_REASONING_EFFORT_ENV).as_deref(),
        )?;
        apply_env_context_seeds(
            &mut overrides,
            std::env::var_os(MODEL_CONTEXT_WINDOW_ENV).as_deref(),
            std::env::var_os(AUTO_COMPACT_TOKEN_LIMIT_ENV).as_deref(),
            std::env::var_os(AUTO_COMPACT_RATIO_ENV).as_deref(),
        )?;
        Ok(overrides)
    }
}

/// Fork: `CODEX_MODEL_REASONING_EFFORT` seeds `model_reasoning_effort` at a
/// precedence between config.toml and an explicit `-c` override. Values are
/// validated against the canonical effort levels — an unknown value errors
/// here instead of silently becoming a Custom effort that some wires drop.
const MODEL_REASONING_EFFORT_ENV: &str = "CODEX_MODEL_REASONING_EFFORT";

/// Fork: `CODEX_MODEL_CONTEXT_WINDOW` seeds `model_context_window`,
/// `CODEX_AUTO_COMPACT_TOKEN_LIMIT` seeds `model_auto_compact_token_limit`,
/// and `CODEX_AUTO_COMPACT_RATIO` seeds `model_auto_compact_ratio` — each at
/// a precedence between config.toml and an explicit `-c` override. Container
/// deployments set these instead of baking vendor-specific token counts into
/// config files.
const MODEL_CONTEXT_WINDOW_ENV: &str = "CODEX_MODEL_CONTEXT_WINDOW";
const AUTO_COMPACT_TOKEN_LIMIT_ENV: &str = "CODEX_AUTO_COMPACT_TOKEN_LIMIT";
const AUTO_COMPACT_RATIO_ENV: &str = "CODEX_AUTO_COMPACT_RATIO";

fn apply_env_context_seeds(
    overrides: &mut Vec<(String, Value)>,
    context_window: Option<&OsStr>,
    token_limit: Option<&OsStr>,
    ratio: Option<&OsStr>,
) -> Result<(), String> {
    seed_env_integer(
        overrides,
        MODEL_CONTEXT_WINDOW_ENV,
        "model_context_window",
        context_window,
    )?;
    seed_env_integer(
        overrides,
        AUTO_COMPACT_TOKEN_LIMIT_ENV,
        "model_auto_compact_token_limit",
        token_limit,
    )?;
    seed_env_ratio(
        overrides,
        AUTO_COMPACT_RATIO_ENV,
        "model_auto_compact_ratio",
        ratio,
    )?;
    Ok(())
}

/// Shared preamble for one environment-seeded override: skips when an
/// explicit `-c` already set the key, and normalizes the raw value. `Ok(None)`
/// means "nothing to seed" (unset or blank).
fn env_override_text<'a>(
    overrides: &[(String, Value)],
    config_key: &str,
    env_name: &str,
    value: Option<&'a OsStr>,
) -> Result<Option<&'a str>, String> {
    if overrides.iter().any(|(key, _)| key == config_key) {
        // An explicit -c wins over the environment.
        return Ok(None);
    }
    let Some(value) = value else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .ok_or_else(|| format!("Invalid {env_name}: expected Unicode text"))?
        .trim();
    Ok((!raw.is_empty()).then_some(raw))
}

fn seed_env_integer(
    overrides: &mut Vec<(String, Value)>,
    env_name: &str,
    config_key: &str,
    value: Option<&OsStr>,
) -> Result<(), String> {
    let Some(raw) = env_override_text(overrides, config_key, env_name, value)? else {
        return Ok(());
    };
    let invalid =
        || format!("Invalid {env_name} value {raw:?}; expected a positive integer token count");
    let parsed: i64 = raw.parse().map_err(|_| invalid())?;
    if parsed <= 0 {
        return Err(invalid());
    }
    overrides.push((config_key.into(), Value::Integer(parsed)));
    Ok(())
}

fn seed_env_ratio(
    overrides: &mut Vec<(String, Value)>,
    env_name: &str,
    config_key: &str,
    value: Option<&OsStr>,
) -> Result<(), String> {
    let Some(raw) = env_override_text(overrides, config_key, env_name, value)? else {
        return Ok(());
    };
    let invalid = || format!("Invalid {env_name} value {raw:?}; expected a ratio within (0, 1]");
    let parsed: f64 = raw
        .parse()
        .map_err(|_| format!("Invalid {env_name} value {raw:?}; expected a ratio like 0.8"))?;
    if !(0.0..=1.0).contains(&parsed) || parsed == 0.0 {
        return Err(invalid());
    }
    overrides.push((config_key.into(), Value::Float(parsed)));
    Ok(())
}

fn apply_env_effort_override(
    overrides: &mut Vec<(String, Value)>,
    env_value: Option<&OsStr>,
) -> Result<(), String> {
    let Some(text) = env_override_text(
        overrides,
        "model_reasoning_effort",
        MODEL_REASONING_EFFORT_ENV,
        env_value,
    )?
    else {
        return Ok(());
    };
    let canonical = text.to_ascii_lowercase();
    const VALID: [&str; 9] = [
        "none",
        "minimal",
        "low",
        "medium",
        "high",
        "xhigh",
        "max",
        "ultra",
        "persistent",
    ];
    if !VALID.contains(&canonical.as_str()) {
        return Err(format!(
            "Invalid {MODEL_REASONING_EFFORT_ENV} value {text:?}; expected one of {}",
            VALID.join("/")
        ));
    }
    overrides.push(("model_reasoning_effort".into(), Value::String(canonical)));
    Ok(())
}

fn canonicalize_override_key(key: &str) -> String {
    if key == "use_legacy_landlock" {
        "features.use_legacy_landlock".to_string()
    } else {
        key.to_string()
    }
}

fn parse_toml_value(raw: &str) -> Result<Value, toml::de::Error> {
    let wrapped = format!("_x_ = {raw}");
    let table: toml::Table = toml::from_str(&wrapped)?;
    table
        .get("_x_")
        .cloned()
        .ok_or_else(|| SerdeError::custom("missing sentinel key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_scalar() {
        let v = parse_toml_value("42").expect("parse");
        assert_eq!(v.as_integer(), Some(42));
    }

    #[test]
    fn parses_bool() {
        let true_literal = parse_toml_value("true").expect("parse");
        assert_eq!(true_literal.as_bool(), Some(true));

        let false_literal = parse_toml_value("false").expect("parse");
        assert_eq!(false_literal.as_bool(), Some(false));
    }

    #[test]
    fn fails_on_unquoted_string() {
        assert!(parse_toml_value("hello").is_err());
    }

    #[test]
    fn parses_array() {
        let v = parse_toml_value("[1, 2, 3]").expect("parse");
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn canonicalizes_use_legacy_landlock_alias() {
        let overrides = CliConfigOverrides {
            raw_overrides: vec!["use_legacy_landlock=true".to_string()],
        };
        let parsed = overrides.parse_overrides().expect("parse_overrides");
        assert_eq!(parsed[0].0.as_str(), "features.use_legacy_landlock");
        assert_eq!(parsed[0].1.as_bool(), Some(true));
    }

    #[test]
    fn prepends_root_overrides() {
        let mut subcommand_overrides = CliConfigOverrides {
            raw_overrides: vec![r#"model="gpt-5.2""#.to_string()],
        };
        subcommand_overrides.prepend_root_overrides(CliConfigOverrides {
            raw_overrides: vec![r#"model="gpt-5.1""#.to_string()],
        });

        assert_eq!(
            subcommand_overrides.raw_overrides,
            vec![
                r#"model="gpt-5.1""#.to_string(),
                r#"model="gpt-5.2""#.to_string(),
            ]
        );
    }

    #[test]
    fn parses_inline_table() {
        let v = parse_toml_value("{a = 1, b = 2}").expect("parse");
        let tbl = v.as_table().expect("table");
        assert_eq!(tbl.get("a").unwrap().as_integer(), Some(1));
        assert_eq!(tbl.get("b").unwrap().as_integer(), Some(2));
    }
}

#[cfg(test)]
#[path = "env_effort_tests.rs"]
mod env_effort_tests;

#[cfg(test)]
#[path = "env_context_seeds_tests.rs"]
mod env_context_seeds_tests;
