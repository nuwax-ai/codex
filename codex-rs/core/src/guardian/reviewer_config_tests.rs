use super::reachable_review_model_id;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::built_in_model_providers;
use pretty_assertions::assert_eq;

#[test]
fn first_party_providers_keep_their_supported_reviewer() {
    for (id, info) in built_in_model_providers(/*openai_base_url*/ None) {
        if !info.is_first_party() {
            continue;
        }
        let provider = create_model_provider(info.clone(), /*auth_manager*/ None);
        let preferred = provider.approval_review_preferred_model();
        assert_eq!(
            reachable_review_model_id(&info, preferred, "active-model"),
            preferred,
            "{id}"
        );
    }
}

#[test]
fn custom_provider_identity_selects_the_active_reviewer() {
    let provider = ModelProviderInfo {
        name: "OpenAI".to_string(),
        provider_id: Some("mimo".to_string()),
        ..Default::default()
    };
    assert_eq!(
        reachable_review_model_id(&provider, "gpt-5.6-luna", "mimo-v2.6-flash"),
        "mimo-v2.6-flash"
    );
}
