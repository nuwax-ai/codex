use super::*;

#[test]
fn local_environment_overrides_base_file_without_losing_other_values() {
    let root = tempfile::tempdir().expect("temporary config root");
    std::fs::write(
        root.path().join(".env"),
        "LIVE_VENDOR_MODEL=base\nBASE_ONLY='retained'\n",
    )
    .expect("base config");
    std::fs::write(
        root.path().join(".env.local"),
        "# local overrides\nLIVE_VENDOR_MODEL=\"local\"\nLOCAL_ONLY=present\n",
    )
    .expect("local config");

    assert_eq!(
        load_env_files_from(root.path()),
        HashMap::from([
            ("LIVE_VENDOR_MODEL".into(), "local".into()),
            ("BASE_ONLY".into(), "retained".into()),
            ("LOCAL_ONLY".into(), "present".into()),
        ])
    );
}

#[test]
fn replay_keeps_anthropic_scenarios_active_without_credentials_or_urls() {
    let config = vendor_from_env(&|_| None, "glm", CassetteMode::Replay)
        .expect("offline vendor remains active");
    assert_eq!(
        (
            config.vendor,
            config.api_key,
            config.base_url,
            config.anthropic_base_url,
            config.responses_base_url,
            config.model,
        ),
        (
            "glm".into(),
            "(replay-placeholder)".into(),
            "(replay-placeholder-url)".into(),
            Some("(replay-placeholder-anthropic-url)".into()),
            // An unconfigured Responses endpoint skips in every mode.
            None,
            "(replay-placeholder-model)".into(),
        )
    );
}

#[test]
fn live_configuration_never_invents_an_anthropic_endpoint() {
    let values = HashMap::from([
        ("LIVE_VENDOR_API_KEY", "dummy"),
        ("LIVE_VENDOR_CHAT_URL", "http://127.0.0.1/v1"),
        ("LIVE_VENDOR_MODEL", "model"),
    ]);
    let lookup = |key: &str| values.get(key).map(|value| (*value).to_string());
    let config = vendor_from_env(&lookup, "glm", CassetteMode::Off).expect("live config");
    assert_eq!(config.anthropic_base_url, None);
    assert!(vendor_from_env(&|_| None, "glm", CassetteMode::Off).is_none());
}
