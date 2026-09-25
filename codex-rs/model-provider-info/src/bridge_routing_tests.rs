use super::*;
use pretty_assertions::assert_eq;

#[test]
fn route_selection_distinguishes_builtin_identity_from_display_name() {
    for provider in [
        ModelProviderInfo::create_openai_provider(None),
        ModelProviderInfo::create_amazon_bedrock_provider(None),
        ModelProviderInfo::create_amazon_bedrock_runtime_provider(None),
    ] {
        assert!(!provider.uses_chat_bridge());
        assert!(
            ModelProviderInfo {
                experimental_bridge: Some(ChatBridge::Rig),
                ..provider
            }
            .uses_chat_bridge()
        );
    }
    let custom = ModelProviderInfo {
        name: "OpenAI".into(),
        provider_id: Some("my-gateway".into()),
        ..Default::default()
    };
    assert!(custom.uses_chat_bridge());
    assert!(
        !ModelProviderInfo {
            experimental_bridge: Some(ChatBridge::Native),
            ..custom
        }
        .uses_chat_bridge()
    );
    for wire_api in [WireApi::Chat, WireApi::Anthropic] {
        assert_eq!(
            ModelProviderInfo {
                wire_api,
                ..Default::default()
            }
            .uses_chat_bridge(),
            true
        );
    }
}
