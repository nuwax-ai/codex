//! Hermetic tests for the hook-trust bypass env seed. Values are injected as
//! parameters, never read from the process environment.

use super::*;
use pretty_assertions::assert_eq;

fn seed(value: Option<&str>) -> (Option<OsString>, &'static str) {
    (value.map(OsString::from), BYPASS_HOOK_TRUST_ENV)
}

#[test]
fn true_enables_the_bypass() {
    let mut overrides = Vec::new();
    seed_env_hook_trust_bypass(&mut overrides, seed(Some("true"))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("bypass_hook_trust".into(), Value::Boolean(true))]
    );
}

#[test]
fn false_is_a_seed_too_so_the_environment_can_turn_a_config_grant_off() {
    // A deployment may set `bypass_hook_trust = true` in config.toml and
    // disable it for one run from the environment; the seed must carry the
    // explicit `false`, not skip it.
    let mut overrides = Vec::new();
    seed_env_hook_trust_bypass(&mut overrides, seed(Some("FALSE"))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("bypass_hook_trust".into(), Value::Boolean(false))]
    );
}

#[test]
fn explicit_override_wins_over_the_environment() {
    let mut overrides = vec![("bypass_hook_trust".to_string(), Value::Boolean(false))];
    seed_env_hook_trust_bypass(&mut overrides, seed(Some("true"))).expect("seeds");
    assert_eq!(
        overrides,
        vec![("bypass_hook_trust".to_string(), Value::Boolean(false))]
    );
}

#[test]
fn anything_but_true_or_false_is_rejected_naming_the_variable() {
    // This grant executes commands; `1`/`yes`/`on` are refused rather than
    // guessed at, so no accidental truthy string turns it on.
    for invalid in ["1", "yes", "on", "0", "enabled", ""] {
        if invalid.is_empty() {
            // Blank counts as unset (see `env_override_text`), not as a grant.
            let mut overrides = Vec::new();
            seed_env_hook_trust_bypass(&mut overrides, seed(Some(invalid))).expect("blank");
            assert!(overrides.is_empty());
            continue;
        }
        let mut overrides = Vec::new();
        let error = seed_env_hook_trust_bypass(&mut overrides, seed(Some(invalid)))
            .expect_err("invalid value");
        assert!(error.contains(BYPASS_HOOK_TRUST_ENV), "{error}");
        assert!(
            overrides.is_empty(),
            "nothing is seeded for an invalid value"
        );
    }
}

#[test]
fn unset_is_a_no_op() {
    let mut overrides = Vec::new();
    seed_env_hook_trust_bypass(&mut overrides, seed(None)).expect("unset");
    assert!(overrides.is_empty());
}
