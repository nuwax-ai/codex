//! Hermetic tests for the `CODEX_TOOL_SEARCH` env seed. Values are injected
//! as parameters, never read from the process environment.

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn seeds_the_nested_config_key() {
    let mut overrides = Vec::new();
    seed_env_tool_search(&mut overrides, Some(OsStr::new("false"))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("tools.tool_search".into(), Value::Boolean(false))]
    );
}

#[test]
fn accepts_case_insensitive_true_and_false() {
    let mut overrides = Vec::new();
    seed_env_tool_search(&mut overrides, Some(OsStr::new("TRUE"))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("tools.tool_search".into(), Value::Boolean(true))]
    );

    let mut overrides = Vec::new();
    seed_env_tool_search(&mut overrides, Some(OsStr::new(" False "))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("tools.tool_search".into(), Value::Boolean(false))]
    );
}

#[test]
fn explicit_override_wins_over_environment() {
    let mut overrides = vec![("tools.tool_search".to_string(), Value::Boolean(true))];
    seed_env_tool_search(&mut overrides, Some(OsStr::new("false"))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("tools.tool_search".into(), Value::Boolean(true))]
    );
}

#[test]
fn missing_or_empty_values_are_noops() {
    let mut overrides = Vec::new();
    seed_env_tool_search(&mut overrides, None).expect("seeds");
    seed_env_tool_search(&mut overrides, Some(OsStr::new(""))).expect("seeds");
    seed_env_tool_search(&mut overrides, Some(OsStr::new("  "))).expect("seeds");
    assert_eq!(overrides, Vec::<(String, Value)>::new());
}

#[test]
fn invalid_values_fail_fast_with_the_env_name() {
    let mut overrides = Vec::new();
    let err = seed_env_tool_search(&mut overrides, Some(OsStr::new("off"))).expect_err("non-bool");
    assert!(err.contains("CODEX_TOOL_SEARCH"), "unexpected: {err}");
    assert!(overrides.is_empty(), "failed seed must not push a value");
}
