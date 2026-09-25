use super::*;
use pretty_assertions::assert_eq;

#[test]
fn provider_switch_keeps_native_ciphertext_but_filters_bridge_reasoning() -> anyhow::Result<()> {
    use codex_protocol::models::ReasoningItemContent;
    let native = ResponseItem::Reasoning {
        id: None,
        summary: vec![],
        content: None,
        encrypted_content: Some("native-ciphertext".into()),
        internal_chat_message_metadata_passthrough: None,
    };
    let legacy = ResponseItem::Reasoning {
        id: None,
        summary: vec![],
        content: Some(vec![ReasoningItemContent::ReasoningText {
            text: "legacy thinking".into(),
        }]),
        encrypted_content: Some("legacy thinking".into()),
        internal_chat_message_metadata_passthrough: None,
    };
    let envelope = ResponseItem::Reasoning {
        id: None,
        summary: vec![],
        content: None,
        encrypted_content: Some("codex-rig-reasoning-v1:opaque".into()),
        internal_chat_message_metadata_passthrough: None,
    };
    for bridge in [
        None,
        Some(codex_model_provider_info::ChatBridge::Rig),
        Some(codex_model_provider_info::ChatBridge::Genai),
    ] {
        let mut client = test_model_client(SessionSource::Cli);
        Arc::get_mut(&mut client.state)
            .expect("unique state")
            .provider = create_model_provider(
            ModelProviderInfo {
                experimental_bridge: bridge,
                ..ModelProviderInfo::create_openai_provider(None)
            },
            None,
        );
        let original = vec![native.clone(), legacy.clone(), envelope.clone()];
        let prompt = Prompt {
            input: original.clone(),
            ..Default::default()
        };
        let metadata = test_responses_metadata_for_client(
            &client,
            None,
            "turn:0".into(),
            None,
            TestCodexResponsesRequestKind::Turn,
        );
        let mut model = test_model_info();
        model.use_responses_lite = false;
        let request = client.build_responses_request(
            &prompt,
            &model,
            None,
            codex_protocol::config_types::ReasoningSummary::None,
            None,
            &metadata,
            true,
        )?;
        let expected = match bridge {
            None => vec![native.clone()],
            Some(codex_model_provider_info::ChatBridge::Genai) => {
                vec![native.clone(), legacy.clone()]
            }
            Some(_) => original.clone(),
        };
        assert_eq!(request.input, expected);
        assert_eq!(
            prompt.input, original,
            "request adaptation must not rewrite saved history"
        );
    }
    Ok(())
}
