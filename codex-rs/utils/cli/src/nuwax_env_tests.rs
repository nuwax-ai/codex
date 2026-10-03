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
        ..Default::default()
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
        // Hostless authority that the old hand-rolled split accepted.
        (Some("http://@"), Some("chat"), Some("k"), "NUWAX_BASE_URL"),
        // Credentials in the userinfo never reach an endpoint.
        (
            Some("https://user:secret@gw.example"),
            Some("chat"),
            Some("k"),
            "NUWAX_BASE_URL",
        ),
        (
            Some("https://gw.example:notaport"),
            Some("chat"),
            Some("k"),
            "NUWAX_BASE_URL",
        ),
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
                ..Default::default()
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
    for existing in [
        vec![(
            "model_providers.nuwax_env".into(),
            Value::Table(Default::default()),
        )],
        vec![(
            "model_providers.nuwax_env.env_key".into(),
            Value::String("OLD_KEY".into()),
        )],
        vec![(
            "model_providers.nuwax_env.base_url".into(),
            Value::String("https://old.example".into()),
        )],
        vec![(
            "model_providers".into(),
            Value::Table(
                [("nuwax_env".into(), Value::Table(Default::default()))]
                    .into_iter()
                    .collect(),
            ),
        )],
    ] {
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

#[test]
fn replaced_blank_model_is_not_validated_for_active_or_inactive_group() {
    for active in [false, true] {
        for typed in [false, true] {
            let existing = if typed {
                vec![]
            } else {
                vec![("model".to_string(), Value::String("explicit".into()))]
            };
            let seeds = nuwax_env_overrides(
                input(
                    Some(" "),
                    active.then_some("https://gw.example"),
                    active.then_some("chat"),
                    active.then_some("k"),
                ),
                typed.then_some("explicit"),
                /*cli_provider*/ None,
                &existing,
            )
            .expect("unused model must not fail startup");
            assert!(seeds.iter().all(|(key, _)| key != "model"));
        }
    }
}

#[cfg(unix)]
#[test]
fn replaced_non_unicode_model_is_not_validated() {
    use std::os::unix::ffi::OsStringExt;

    let mut input = input(None, Some("https://gw.example"), Some("chat"), Some("k"));
    input.model = Some(OsString::from_vec(vec![0xff]));
    let seeds = nuwax_env_overrides(
        input,
        /*cli_model*/ Some("explicit"),
        /*cli_provider*/ None,
        &[],
    )
    .expect("explicit model replaces non-Unicode environment value");
    assert!(seeds.iter().all(|(key, _)| key != "model"));
}

#[test]
fn repeated_provider_overrides_adopt_only_the_final_selection() {
    for (first, last, expected_keys) in [
        (
            "other",
            "nuwax_env",
            vec!["model_providers.nuwax_env", "model"],
        ),
        ("nuwax_env", "other", vec![]),
    ] {
        let existing = vec![
            ("model_provider".into(), Value::String(first.into())),
            ("model_provider".into(), Value::String(last.into())),
        ];
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
        .expect("last provider override determines environment adoption");
        assert_eq!(
            seeds
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            expected_keys
        );
    }
}

#[test]
fn optional_output_cap_is_a_numeric_seed_for_chat_and_anthropic() {
    for wire in ["chat", "anthropic", "responses"] {
        for cap in [1, 4096, i64::MAX] {
            let mut environment = input(
                Some("m1"),
                Some("https://gw.example"),
                Some(wire),
                Some("private-key"),
            );
            environment.max_output_tokens = Some(format!(" {cap} ").into());
            let actual = nuwax_env_overrides(
                environment,
                /*cli_model*/ None,
                /*cli_provider*/ None,
                &[],
            )
            .expect("supported output cap");
            let mut expected = provider_seeds(wire);
            expected[0]
                .1
                .as_table_mut()
                .expect("provider seed")
                .insert("max_output_tokens".into(), Value::Integer(cap));
            assert_eq!(actual, expected);
            assert!(
                !format!("{actual:?}").contains("private-key"),
                "credentials remain an environment reference"
            );
        }
    }
}

#[test]
fn adopted_output_cap_rejects_blank_nonpositive_nonnumeric_and_out_of_range_values() {
    for raw in [
        "",
        " ",
        "0",
        "-1",
        "1.5",
        "not-a-number",
        "9223372036854775808",
    ] {
        let mut environment = input(
            Some("m1"),
            Some("https://gw.example"),
            Some("chat"),
            Some("private-key"),
        );
        environment.max_output_tokens = Some(raw.into());
        let error = nuwax_env_overrides(
            environment,
            /*cli_model*/ None,
            /*cli_provider*/ None,
            &[],
        )
        .expect_err("invalid output cap must fail fast");
        assert!(error.contains("NUWAX_MAX_OUTPUT_TOKENS"), "{error}");
        assert!(
            !error.contains("private-key"),
            "errors never carry credentials"
        );
    }
}

#[cfg(unix)]
#[test]
fn adopted_output_cap_rejects_non_unicode() {
    use std::os::unix::ffi::OsStringExt;
    let mut environment = input(
        Some("m1"),
        Some("https://gw.example"),
        Some("anthropic"),
        Some("private-key"),
    );
    environment.max_output_tokens = Some(OsString::from_vec(vec![0xff]));
    let error = nuwax_env_overrides(
        environment,
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect_err("non-Unicode output cap");
    assert_eq!(
        error,
        "Invalid NUWAX_MAX_OUTPUT_TOKENS: expected Unicode text"
    );
}

#[test]
fn standalone_output_cap_requires_the_complete_group_but_unselected_groups_are_ignored() {
    let environment = NuwaxEnvInput {
        max_output_tokens: Some("2048".into()),
        ..Default::default()
    };
    let error = nuwax_env_overrides(
        environment.clone(),
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[],
    )
    .expect_err("standalone cap has no provider owner");
    for variable in [
        "NUWAX_MAX_OUTPUT_TOKENS",
        "NUWAX_BASE_URL",
        "NUWAX_WIRE_API",
        "NUWAX_API_KEY",
    ] {
        assert!(error.contains(variable), "{error}");
    }
    assert_eq!(
        nuwax_env_overrides(
            environment.clone(),
            /*cli_model*/ None,
            /*cli_provider*/ Some("other-provider"),
            &[]
        )
        .expect("explicit provider wins"),
        Vec::new()
    );
    assert_eq!(
        nuwax_env_overrides(
            environment,
            /*cli_model*/ None,
            /*cli_provider*/ None,
            &[(
                "model_provider".into(),
                Value::String("other-provider".into())
            )]
        )
        .expect("explicit -c provider wins"),
        Vec::new()
    );
    let mut unused = input(
        Some("m1"),
        Some("invalid-url"),
        Some("responses"),
        Some("private-key"),
    );
    unused.max_output_tokens = Some("invalid-cap".into());
    assert_eq!(
        nuwax_env_overrides(
            unused,
            /*cli_model*/ None,
            /*cli_provider*/ Some("other-provider"),
            &[]
        )
        .expect("unused invalid options ignored"),
        Vec::new()
    );
}

#[test]
fn cli_cannot_supply_the_reserved_output_cap_field() {
    let mut environment = input(
        Some("m1"),
        Some("https://gw.example"),
        Some("chat"),
        Some("private-key"),
    );
    environment.max_output_tokens = Some("4096".into());
    let error = nuwax_env_overrides(
        environment,
        /*cli_model*/ None,
        /*cli_provider*/ None,
        &[(
            "model_providers.nuwax_env.max_output_tokens".into(),
            Value::Integer(2048),
        )],
    )
    .expect_err("provider fields remain exclusive to the seed layer");
    assert!(error.contains("reserved"), "{error}");
}
