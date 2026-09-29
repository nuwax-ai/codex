//! Hermetic tests for the context-window env seeds. Values are injected as
//! parameters, never read from the process environment.

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn seeds_all_three_context_keys() {
    let mut overrides = Vec::new();
    apply_env_context_seeds(
        &mut overrides,
        Some(OsStr::new("200000")),
        Some(OsStr::new("120000")),
        Some(OsStr::new("0.8")),
    )
    .expect("seeds");
    assert_eq!(
        overrides,
        vec![
            ("model_context_window".into(), Value::Integer(200000)),
            (
                "model_auto_compact_token_limit".into(),
                Value::Integer(120000)
            ),
            ("model_auto_compact_ratio".into(), Value::Float(0.8)),
        ]
    );
}

#[test]
fn explicit_override_wins_over_environment() {
    let mut overrides = vec![("model_context_window".to_string(), Value::Integer(64000))];
    apply_env_context_seeds(
        &mut overrides,
        Some(OsStr::new("200000")),
        Some(OsStr::new("120000")),
        None,
    )
    .expect("seeds");
    assert_eq!(
        overrides,
        vec![
            ("model_context_window".into(), Value::Integer(64000)),
            (
                "model_auto_compact_token_limit".into(),
                Value::Integer(120000)
            ),
        ]
    );
}

#[test]
fn missing_or_empty_values_are_noops() {
    let mut overrides = Vec::new();
    apply_env_context_seeds(
        &mut overrides,
        None,
        Some(OsStr::new("  ")),
        Some(OsStr::new("")),
    )
    .expect("seeds");
    assert_eq!(overrides, Vec::<(String, Value)>::new());
}

#[test]
fn invalid_values_fail_fast_with_the_env_name() {
    let mut overrides = Vec::new();
    let err = apply_env_context_seeds(&mut overrides, Some(OsStr::new("big")), None, None)
        .expect_err("non-numeric window");
    assert!(
        err.contains("CODEX_MODEL_CONTEXT_WINDOW"),
        "unexpected: {err}"
    );

    let mut overrides = Vec::new();
    let err = apply_env_context_seeds(&mut overrides, Some(OsStr::new("0")), None, None)
        .expect_err("zero window");
    assert!(
        err.contains("CODEX_MODEL_CONTEXT_WINDOW"),
        "unexpected: {err}"
    );

    let mut overrides = Vec::new();
    let err = apply_env_context_seeds(&mut overrides, None, None, Some(OsStr::new("1.5")))
        .expect_err("ratio above one");
    assert!(
        err.contains("CODEX_AUTO_COMPACT_RATIO"),
        "unexpected: {err}"
    );

    let mut overrides = Vec::new();
    apply_env_context_seeds(&mut overrides, None, None, Some(OsStr::new("1")))
        .expect("ratio of exactly one is valid");
    assert_eq!(
        overrides,
        vec![("model_auto_compact_ratio".into(), Value::Float(1.0))]
    );
}
