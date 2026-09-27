use super::*;
use pretty_assertions::assert_eq;

#[test]
fn env_effort_seeds_when_not_explicitly_overridden() {
    let mut overrides = vec![("model".to_string(), Value::String("x".into()))];
    apply_env_effort_override(&mut overrides, Some(OsStr::new(" NONE "))).expect("valid");
    assert_eq!(
        overrides,
        vec![
            ("model".to_string(), Value::String("x".into())),
            (
                "model_reasoning_effort".to_string(),
                Value::String("none".into())
            ),
        ]
    );
}

#[test]
fn env_effort_yields_to_explicit_override() {
    let expected = vec![(
        "model_reasoning_effort".to_string(),
        Value::String("high".into()),
    )];
    for raw in ["none", "invalid"] {
        let mut overrides = expected.clone();
        apply_env_effort_override(&mut overrides, Some(OsStr::new(raw))).expect("explicit wins");
        assert_eq!(overrides, expected);
    }
}

#[test]
fn env_effort_rejects_unknown_values_with_the_valid_list() {
    let mut overrides = vec![];
    let error = apply_env_effort_override(&mut overrides, Some(OsStr::new("hight")))
        .expect_err("typo must fail fast");
    assert!(
        error.contains("hight") && error.contains("none/minimal"),
        "{error}"
    );
    assert!(overrides.is_empty());
}

#[test]
fn env_effort_ignores_unset_or_blank() {
    let mut overrides = vec![];
    apply_env_effort_override(&mut overrides, /*env_value*/ None).expect("unset is a no-op");
    apply_env_effort_override(&mut overrides, Some(OsStr::new("   "))).expect("blank is a no-op");
    assert!(overrides.is_empty());
}

#[cfg(any(unix, windows))]
#[test]
fn env_effort_rejects_non_unicode_unless_explicitly_overridden() {
    #[cfg(unix)]
    let raw = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    };
    #[cfg(windows)]
    let raw = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800])
    };
    for explicit in ["", "low"] {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "config_override::env_effort_tests::parse_non_unicode_effort_child",
                "--ignored",
            ])
            .env(MODEL_REASONING_EFFORT_ENV, &raw)
            .env("CODEX_TEST_EXPLICIT_EFFORT", explicit)
            .output()
            .expect("run parser in isolated process");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "parser child failed or matched no tests: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(any(unix, windows))]
#[test]
#[ignore = "subprocess entry point for env_effort_rejects_non_unicode_unless_explicitly_overridden"]
fn parse_non_unicode_effort_child() {
    let explicit = std::env::var("CODEX_TEST_EXPLICIT_EFFORT").expect("test case");
    let parsed = CliConfigOverrides {
        raw_overrides: if explicit.is_empty() {
            vec![]
        } else {
            vec![format!("model_reasoning_effort={explicit}")]
        },
    }
    .parse_overrides();
    let expected = if explicit.is_empty() {
        Err("Invalid CODEX_MODEL_REASONING_EFFORT: expected Unicode text".to_string())
    } else {
        Ok(vec![(
            "model_reasoning_effort".to_string(),
            Value::String(explicit),
        )])
    };
    assert_eq!(parsed, expected);
}
