//! Hermetic tests for the context-window env seeds. Values are injected as
//! parameters, never read from the process environment.

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn seeds_all_three_context_keys() {
    let mut overrides = Vec::new();
    apply_env_context_seeds(
        &mut overrides,
        seed_env(Some("200000"), MODEL_CONTEXT_WINDOW_ENV),
        seed_env(Some("120000"), AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(Some("0.8"), AUTO_COMPACT_RATIO_ENV),
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
        seed_env(Some("200000"), MODEL_CONTEXT_WINDOW_ENV),
        seed_env(Some("120000"), AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(None, AUTO_COMPACT_RATIO_ENV),
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
        seed_env(None, MODEL_CONTEXT_WINDOW_ENV),
        seed_env(Some("  "), AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(Some(""), AUTO_COMPACT_RATIO_ENV),
    )
    .expect("seeds");
    assert_eq!(overrides, Vec::<(String, Value)>::new());
}

#[test]
fn invalid_values_fail_fast_with_the_env_name() {
    let mut overrides = Vec::new();
    let err = apply_env_context_seeds(
        &mut overrides,
        seed_env(Some("big"), MODEL_CONTEXT_WINDOW_ENV),
        seed_env(None, AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(None, AUTO_COMPACT_RATIO_ENV),
    )
    .expect_err("non-numeric window");
    assert!(
        err.contains("CODEX_MODEL_CONTEXT_WINDOW"),
        "unexpected: {err}"
    );

    let mut overrides = Vec::new();
    let err = apply_env_context_seeds(
        &mut overrides,
        seed_env(Some("0"), MODEL_CONTEXT_WINDOW_ENV),
        seed_env(None, AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(None, AUTO_COMPACT_RATIO_ENV),
    )
    .expect_err("zero window");
    assert!(
        err.contains("CODEX_MODEL_CONTEXT_WINDOW"),
        "unexpected: {err}"
    );

    let mut overrides = Vec::new();
    let err = apply_env_context_seeds(
        &mut overrides,
        seed_env(None, MODEL_CONTEXT_WINDOW_ENV),
        seed_env(None, AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(Some("1.5"), AUTO_COMPACT_RATIO_ENV),
    )
    .expect_err("ratio above one");
    assert!(
        err.contains("CODEX_AUTO_COMPACT_RATIO"),
        "unexpected: {err}"
    );

    let mut overrides = Vec::new();
    apply_env_context_seeds(
        &mut overrides,
        seed_env(None, MODEL_CONTEXT_WINDOW_ENV),
        seed_env(None, AUTO_COMPACT_TOKEN_LIMIT_ENV),
        seed_env(Some("1"), AUTO_COMPACT_RATIO_ENV),
    )
    .expect("ratio of exactly one is valid");
    assert_eq!(
        overrides,
        vec![("model_auto_compact_ratio".into(), Value::Float(1.0))]
    );
}
