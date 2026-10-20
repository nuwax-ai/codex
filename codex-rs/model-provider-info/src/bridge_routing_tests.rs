use super::*;
use pretty_assertions::assert_eq;

#[test]
fn route_selection_distinguishes_builtin_identity_from_display_name() {
    for provider in [
        ModelProviderInfo::create_openai_provider(None),
        ModelProviderInfo::create_amazon_bedrock_provider(None),
        ModelProviderInfo::create_amazon_bedrock_runtime_provider(None),
    ] {
        assert!(!provider.uses_model_bridge());
        assert!(
            ModelProviderInfo {
                experimental_bridge: Some(ChatBridge::Rig),
                ..provider
            }
            .uses_model_bridge()
        );
    }
    let custom = ModelProviderInfo {
        name: "OpenAI".into(),
        provider_id: Some("my-gateway".into()),
        ..Default::default()
    };
    assert!(custom.uses_model_bridge());
    assert!(
        !ModelProviderInfo {
            experimental_bridge: Some(ChatBridge::Native),
            ..custom
        }
        .uses_model_bridge()
    );
    for wire_api in [WireApi::Chat, WireApi::Anthropic] {
        assert_eq!(
            ModelProviderInfo {
                wire_api,
                ..Default::default()
            }
            .uses_model_bridge(),
            true
        );
    }
}

#[test]
fn chatgpt_backend_route_keeps_the_native_transport() {
    let routed = ModelProviderInfo {
        name: "OpenAI".into(),
        provider_id: Some("routing".into()),
        base_url: Some("http://127.0.0.1:4000/backend-api/codex".into()),
        requires_openai_auth: true,
        ..Default::default()
    };
    assert!(routed.requires_workspace_routing());
    assert!(!routed.uses_model_bridge());

    // Its Chat and Anthropic variants stay bridged: discovery is a
    // Responses-transport concern.
    for wire_api in [WireApi::Chat, WireApi::Anthropic] {
        assert!(
            ModelProviderInfo {
                wire_api,
                ..routed.clone()
            }
            .uses_model_bridge()
        );
    }

    // A key-bearing gateway under the same display name owns its own
    // credentials, so it keeps bridging.
    assert!(
        ModelProviderInfo {
            env_key: Some("NUWAX_API_KEY".into()),
            ..routed.clone()
        }
        .uses_model_bridge()
    );
    // So does a ChatGPT-auth provider that is not a codex backend route.
    assert!(
        ModelProviderInfo {
            base_url: Some("http://127.0.0.1:4000/v1".into()),
            ..routed.clone()
        }
        .uses_model_bridge()
    );
    // Command and bearer overrides own the credential the same way env_key
    // does; only the unoverridden ChatGPT path needs routing.
    for provider in [
        ModelProviderInfo {
            experimental_bearer_token: Some("t".to_string().into()),
            ..routed.clone()
        },
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("p".into()),
                region: None,
                credential_export: None,
                auth_refresh: None,
            }),
            ..routed
        },
    ] {
        assert!(!provider.requires_workspace_routing());
        assert!(provider.uses_model_bridge());
    }
}
