use super::ConfigToml;
use super::validate_model_providers;
use codex_model_provider_info::ChatBridge;
use codex_model_provider_info::built_in_model_providers;
use codex_model_provider_info::merge_configured_model_providers;
use pretty_assertions::assert_eq;

#[test]
fn stamped_bedrock_overrides_survive_load_validation_and_merge() {
    for id in ["amazon-bedrock", "amazon-bedrock-runtime"] {
        let input = format!(
            "model_provider = \"{id}\"\n[model_providers.{id}.aws]\nregion = \"us-west-2\"\n"
        );
        let config: ConfigToml = toml::from_str(&input).expect("valid override");
        validate_model_providers(&config.model_providers)
            .expect("runtime identity is not an override");
        let providers = merge_configured_model_providers(
            built_in_model_providers(None),
            config.model_providers,
        )
        .unwrap();
        let provider = &providers[id];
        assert_eq!(
            provider.aws.as_ref().and_then(|aws| aws.region.as_deref()),
            Some("us-west-2")
        );
        assert!(!provider.uses_chat_bridge());
    }
}

#[test]
fn builtin_bridge_selection_is_accepted_without_overriding_endpoint() {
    for (id, bridge, expected) in [
        ("ollama", "native", ChatBridge::Native),
        ("lmstudio", "native", ChatBridge::Native),
        ("openai", "rig", ChatBridge::Rig),
    ] {
        let input = format!("[model_providers.{id}]\nexperimental_bridge = \"{bridge}\"\n");
        let config: ConfigToml = toml::from_str(&input).expect("bridge selection");
        let original = built_in_model_providers(None);
        let providers =
            merge_configured_model_providers(original.clone(), config.model_providers).unwrap();
        let mut expected_provider = original[id].clone();
        expected_provider.experimental_bridge = Some(expected);
        assert_eq!(providers[id], expected_provider);
    }
    assert!(toml::from_str::<ConfigToml>("[model_providers.openai]\nname=\"Other\"\nbase_url=\"https://other.test\"\nexperimental_bridge=\"rig\"").is_err());
}
