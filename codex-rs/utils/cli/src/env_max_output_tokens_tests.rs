//! Hermetic tests for the global output-cap env seed. Values are injected as
//! parameters, never read from the process environment.

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn seeds_the_global_output_cap() {
    let mut overrides = Vec::new();
    seed_env_max_output_tokens(
        &mut overrides,
        seed_env(Some("131072"), MAX_OUTPUT_TOKENS_ENV),
    )
    .expect("seeds");
    assert_eq!(
        overrides,
        vec![("model_max_output_tokens".into(), Value::Integer(131_072))]
    );
}

#[test]
fn explicit_override_wins_over_the_environment() {
    let mut overrides = vec![(
        "model_max_output_tokens".to_string(),
        Value::Integer(65_536),
    )];
    seed_env_max_output_tokens(
        &mut overrides,
        seed_env(Some("131072"), MAX_OUTPUT_TOKENS_ENV),
    )
    .expect("seeds");
    assert_eq!(
        overrides,
        vec![("model_max_output_tokens".into(), Value::Integer(65_536))]
    );
}

#[test]
fn invalid_values_fail_naming_the_variable() {
    // An empty value counts as unset (see the seed helper), so it is not an
    // error case here.
    for invalid in ["0", "-1", "wide", "12.5"] {
        let mut overrides = Vec::new();
        let error = seed_env_max_output_tokens(
            &mut overrides,
            seed_env(Some(invalid), MAX_OUTPUT_TOKENS_ENV),
        )
        .expect_err("invalid value");
        assert!(
            error.contains(MAX_OUTPUT_TOKENS_ENV),
            "error must name the variable, got {error}"
        );
        assert!(overrides.is_empty(), "no seed for an invalid value");
    }
}

#[test]
fn unset_is_a_no_op() {
    let mut overrides = Vec::new();
    seed_env_max_output_tokens(&mut overrides, seed_env(None, MAX_OUTPUT_TOKENS_ENV))
        .expect("unset seeds nothing");
    assert!(overrides.is_empty());
}
