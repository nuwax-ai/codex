//! Matrix tests for the NUWAX_* startup group: three-protocol shapes,
//! partial/invalid/non-Unicode values, precedence and credential handling.

use super::*;
use pretty_assertions::assert_eq;
use std::ffi::OsString;

fn input(
    model: Option<&str>,
    base_url: Option<&str>,
    wire_api: Option<&str>,
    api_key: Option<&str>,
) -> NuwaxEnvInput {
    let owned = |value: Option<&str>| value.map(|value| value.to_string().into());
    NuwaxEnvInput {
        model: owned(model),
        base_url: owned(base_url),
        wire_api: owned(wire_api),
        api_key: owned(api_key),
    }
}

fn provider_seeds(wire_api: &str) -> Vec<(String, Value)> {
    vec![
        (
            "model_providers.nuwax_env".to_string(),
            Value::Table(
                toml::from_str::<toml::Table>(&format!(
                    "name = \"nuwax env provider\"\nbase_url = \"https://gw.example\"\n\
                     wire_api = \"{wire_api}\"\nenv_key = \"NUWAX_API_KEY\""
                ))
                .expect("provider table"),
            ),
        ),
        (
            "model_provider".to_string(),
            Value::String("nuwax_env".into()),
        ),
        ("model".to_string(), Value::String("m1".into())),
    ]
}

#[test]
fn full_group_seeds_the_temporary_provider_for_each_wire() {
    for wire_api in ["responses", "chat", "anthropic"] {
        let seeds = nuwax_env_overrides(
            input(
                Some("m1"),
                Some("https://gw.example"),
                Some(wire_api),
                Some("secret-key"),
            ),
            /*cli_model*/ None,
            /*cli_provider*/ None,
            &[],
        )
        .expect("full group seeds");
        assert_eq!(seeds, provider_seeds(wire_api), "wire {wire_api}");
        // The credential is referenced, never embedded.
        let encoded = format!("{seeds:?}");
        assert!(
            !encoded.contains("secret-key"),
            "raw key must not appear: {encoded}"
        );
    }
}

#[test]
fn model_only_selects_an_existing_provider_model() {
    let seeds = nuwax_env_overrides(
        input(Some("glm-5.3"), None, None, None),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect("model-only seeds");
    assert_eq!(
        seeds,
        vec![("model".to_string(), Value::String("glm-5.3".into()))]
    );
}

#[test]
fn inactive_group_with_no_variables_seeds_nothing() {
    let seeds = nuwax_env_overrides(
        input(None, None, None, None),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect("no seeds");
    assert!(seeds.is_empty());
}

#[test]
fn partial_group_fails_fast_naming_the_missing_variables() {
    let error = nuwax_env_overrides(
        input(Some("m1"), Some("https://gw.example"), None, None),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect_err("partial group must fail");
    assert!(
        error.contains("NUWAX_WIRE_API") && error.contains("NUWAX_API_KEY"),
        "error must name the missing variables: {error}"
    );
}

#[test]
fn group_without_any_model_fails_fast() {
    let error = nuwax_env_overrides(
        input(None, Some("https://gw.example"), Some("chat"), Some("k")),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect_err("modelless group must fail");
    assert!(error.contains("requires a model"), "{error}");
}

#[test]
fn cli_model_satisfies_the_model_requirement_without_a_seed() {
    let seeds = nuwax_env_overrides(
        input(None, Some("https://gw.example"), Some("chat"), Some("k")),
        /*cli_model*/ Some("from-flag"),
        /*cli_provider*/ None,
        &[],
    )
    .expect("typed model satisfies the requirement");
    assert!(
        seeds.iter().all(|(key, _)| key != "model"),
        "the typed flag already provides the model; no seed needed: {seeds:?}"
    );
}

#[test]
fn explicit_cli_provider_selection_ignores_and_skips_validation_of_the_group() {
    // Even a partial, invalid group must not break a valid CLI config.
    let seeds = nuwax_env_overrides(
        input(Some("m1"), Some("not a url"), Some("bogus"), None),
        /*cli_model*/ None,
        /*cli_provider*/ Some("openai"),
        &[],
    )
    .expect("group is irrelevant under an explicit provider selection");
    assert!(seeds.is_empty());
}

#[test]
fn explicit_dash_c_provider_selection_behaves_like_the_typed_flag() {
    let existing = vec![(
        "model_provider".to_string(),
        Value::String("my-gateway".into()),
    )];
    let seeds = nuwax_env_overrides(
        input(
            Some("m1"),
            Some("https://gw.example"),
            Some("chat"),
            Some("k"),
        ),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &existing,
    )
    .expect("explicit -c provider makes the group irrelevant");
    assert!(seeds.is_empty());
}

#[test]
fn explicit_dash_c_selecting_the_reserved_id_uses_the_full_group() {
    let existing = vec![(
        "model_provider".to_string(),
        Value::String("nuwax_env".into()),
    )];
    let seeds = nuwax_env_overrides(
        input(
            Some("m1"),
            Some("https://gw.example"),
            Some("anthropic"),
            Some("k"),
        ),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &existing,
    )
    .expect("reserved-id selection uses the group");
    assert_eq!(
        seeds
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["model_providers.nuwax_env", "model"]
    );
}

#[test]
fn invalid_values_are_rejected_with_field_names_only() {
    for (base_url, wire_api, api_key, expected) in [
        (
            Some("ftp://gw.example"),
            Some("chat"),
            Some("k"),
            "NUWAX_BASE_URL",
        ),
        (Some("https://"), Some("chat"), Some("k"), "NUWAX_BASE_URL"),
        (Some("  "), Some("chat"), Some("k"), "NUWAX_BASE_URL"),
        (
            Some("https://gw.example"),
            Some("websocket"),
            Some("k"),
            "NUWAX_WIRE_API",
        ),
        (
            Some("https://gw.example"),
            Some("chat"),
            Some(" "),
            "NUWAX_API_KEY",
        ),
        (None, None, None, "NUWAX_MODEL"),
    ] {
        let error = nuwax_env_overrides(
            input(Some(" "), base_url, wire_api, api_key),
            /*cli_model*/ None,
            /*cli_provider*/ None,
            &[],
        )
        .expect_err("invalid values must fail the group");
        assert!(error.contains(expected), "expected {expected} in {error}");
        assert!(
            !error.contains("secret"),
            "errors never embed values: {error}"
        );
    }
}

#[test]
fn non_unicode_values_fail_with_variable_names() {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0xff, 0xfe]);
        let error = nuwax_env_overrides(
            NuwaxEnvInput {
                model: None,
                base_url: Some(bad),
                wire_api: Some(OsString::from("chat")),
                api_key: Some(OsString::from("k")),
            },
            /*cli_model*/ None,
            /*cli_provider*/ None,
            &[],
        )
        .expect_err("non-Unicode base URL must fail");
        assert!(error.contains("NUWAX_BASE_URL"), "{error}");
    }
}

#[test]
fn reserved_provider_id_conflict_is_a_hard_error() {
    let existing = vec![(
        "model_providers.nuwax_env".to_string(),
        Value::String("must be a table in reality".into()),
    )];
    let error = nuwax_env_overrides(
        input(
            Some("m1"),
            Some("https://gw.example"),
            Some("chat"),
            Some("k"),
        ),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &existing,
    )
    .expect_err("reserved id collision must fail");
    assert!(error.contains("reserved"), "{error}");
}

#[test]
fn explicit_dash_c_model_wins_over_the_environment_model() {
    let existing = vec![("model".to_string(), Value::String("from-c".into()))];
    let seeds = nuwax_env_overrides(
        input(
            Some("m1"),
            Some("https://gw.example"),
            Some("chat"),
            Some("k"),
        ),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &existing,
    )
    .expect("explicit -c model must not be duplicated");
    assert!(
        seeds.iter().all(|(key, _)| key != "model"),
        "-c model wins; the environment must not add a competing seed: {seeds:?}"
    );
}
