//! A reply naming another model is a reroute only on OpenAI's own backend.

use super::server_model_mismatch_is_a_reroute;
use codex_model_provider_info::AMAZON_BEDROCK_PROVIDER_ID;
use codex_model_provider_info::AMAZON_BEDROCK_RUNTIME_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::OPENAI_PROVIDER_NAME;

const REQUESTED: &str = "GLM-5.3-Flash";

fn provider(provider_id: Option<&str>) -> ModelProviderInfo {
    ModelProviderInfo {
        provider_id: provider_id.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn openai_naming_another_model_is_a_reroute() {
    assert!(server_model_mismatch_is_a_reroute(
        &provider(Some(OPENAI_PROVIDER_ID)),
        REQUESTED,
        "gpt-5.2"
    ));
}

#[test]
fn the_same_model_in_another_case_is_not_a_mismatch() {
    assert!(!server_model_mismatch_is_a_reroute(
        &provider(Some(OPENAI_PROVIDER_ID)),
        REQUESTED,
        "glm-5.3-flash"
    ));
    assert!(!server_model_mismatch_is_a_reroute(
        &provider(Some(OPENAI_PROVIDER_ID)),
        REQUESTED,
        REQUESTED
    ));
}

#[test]
fn third_party_gateways_naming_another_model_is_not_a_reroute() {
    // A snapshot or alias name, or the upstream channel's own model name, is
    // how gateways reply; none of them rerouted anything.
    let other = "glm-5.3-flash-251011";
    for provider_id in [None, Some("mimo"), Some("output-cap-test")] {
        assert!(
            !server_model_mismatch_is_a_reroute(&provider(provider_id), REQUESTED, other),
            "provider {provider_id:?}"
        );
    }
}

#[test]
fn the_managed_cloud_transports_do_not_reroute_this_way() {
    for provider_id in [
        AMAZON_BEDROCK_PROVIDER_ID,
        AMAZON_BEDROCK_RUNTIME_PROVIDER_ID,
    ] {
        assert!(
            !server_model_mismatch_is_a_reroute(&provider(Some(provider_id)), REQUESTED, "gpt-5.2"),
            "provider {provider_id}"
        );
    }
}

#[test]
fn the_display_name_does_not_make_a_provider_openai() {
    // `is_openai()` reads the operator-chosen name; the gate must not, or any
    // gateway described as "OpenAI" would raise the alarm.
    let named_openai = ModelProviderInfo {
        provider_id: Some("corp-gateway".into()),
        name: OPENAI_PROVIDER_NAME.into(),
        ..Default::default()
    };
    assert!(named_openai.is_openai());
    assert!(!server_model_mismatch_is_a_reroute(
        &named_openai,
        REQUESTED,
        "gpt-5.2"
    ));
}
