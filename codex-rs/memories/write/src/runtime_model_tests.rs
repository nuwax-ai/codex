use super::reachable_memory_model;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::built_in_model_providers;
use pretty_assertions::assert_eq;

#[test]
fn first_party_providers_keep_their_supported_memory_models() {
    for (id, info) in built_in_model_providers(/*openai_base_url*/ None) {
        if !info.is_first_party() {
            continue;
        }
        let provider = create_model_provider(info.clone(), /*auth_manager*/ None);
        for preferred in [
            provider.memory_extraction_preferred_model(),
            provider.memory_consolidation_preferred_model(),
        ] {
            assert_eq!(
                reachable_memory_model(
                    &info,
                    /*configured*/ None,
                    preferred,
                    Some("active-model"),
                ),
                preferred,
                "{id}"
            );
        }
    }
}

#[test]
fn explicit_memory_models_override_provider_defaults_and_active_models() {
    for id in ["openai", "amazon-bedrock", "amazon-bedrock-runtime", "mimo"] {
        let provider = ModelProviderInfo {
            provider_id: Some(id.to_string()),
            ..Default::default()
        };
        assert_eq!(
            reachable_memory_model(
                &provider,
                Some("custom-extractor"),
                "preferred-model",
                Some("active-model"),
            ),
            "custom-extractor"
        );
    }
}

#[test]
fn custom_provider_identity_selects_the_active_memory_model() {
    let provider = ModelProviderInfo {
        name: "OpenAI".to_string(),
        provider_id: Some("mimo".to_string()),
        ..Default::default()
    };
    assert_eq!(
        reachable_memory_model(
            &provider,
            /*configured*/ None,
            "gpt-5.6-luna",
            Some("mimo-v2.6-flash"),
        ),
        "mimo-v2.6-flash"
    );
}
