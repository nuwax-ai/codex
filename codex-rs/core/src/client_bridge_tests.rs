use super::*;
use pretty_assertions::assert_eq;

#[test]
fn responses_requests_clear_hosted_replay_payloads_without_changing_history() -> anyhow::Result<()>
{
    let original: ResponseItem = serde_json::from_value(serde_json::json!({
        "type":"web_search_call", "id":"search_saved", "status":"completed",
        "action":{"type":"search", "query":"saved query"},
        "wire_blocks":[{"type":"server_tool_use", "id":"srv_saved", "name":"web_search", "input":{"query":"saved query"}}]
    }))?;
    for (wire_api, bridge) in [
        (
            WireApi::Responses,
            codex_model_provider_info::ChatBridge::Native,
        ),
        (
            WireApi::Responses,
            codex_model_provider_info::ChatBridge::Rig,
        ),
        (
            WireApi::Anthropic,
            codex_model_provider_info::ChatBridge::Rig,
        ),
    ] {
        let mut client = test_model_client(SessionSource::Cli);
        Arc::get_mut(&mut client.state)
            .expect("unique state")
            .provider = create_model_provider(
            ModelProviderInfo {
                wire_api,
                experimental_bridge: Some(bridge),
                ..Default::default()
            },
            None,
        );
        let prompt = Prompt {
            input: vec![original.clone()],
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
            false,
        )?;
        let mut expected = original.clone();
        if wire_api == WireApi::Responses
            && let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut expected
        {
            *wire_blocks = None;
        }
        assert_eq!(request.input, vec![expected]);
        assert_eq!(prompt.input, vec![original.clone()]);
    }
    Ok(())
}

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

#[test]
#[cfg(feature = "rust-rig")]
fn rig_reasoning_envelope_prefix_is_v1_mirror() {
    // core's genai-only fallback in `is_rig_reasoning_envelope` hardcodes
    // this prefix; if the bridge envelope format ever versions up, update
    // both sides (grep "codex-rig-reasoning").
    assert_eq!(
        codex_rust_rig_bridge::REPLAY_PREFIX,
        "codex-rig-reasoning-v1:"
    );
}

#[tokio::test]
#[cfg(not(any(feature = "rust-genai", feature = "rust-rig")))]
async fn missing_bridge_features_reject_before_native_responses_dispatch() {
    use codex_model_provider_info::ChatBridge;

    let server = MockServer::start().await;
    for (wire_api, experimental_bridge) in [
        (WireApi::Responses, None),
        (WireApi::Responses, Some(ChatBridge::Rig)),
        (WireApi::Responses, Some(ChatBridge::Genai)),
        (WireApi::Chat, None),
        (WireApi::Anthropic, None),
    ] {
        let mut client = test_model_client(SessionSource::Cli);
        Arc::get_mut(&mut client.state)
            .expect("test client has unique state")
            .provider = create_model_provider(
            ModelProviderInfo {
                name: "bridge-required".into(),
                base_url: Some(format!("{}/v1", server.uri())),
                wire_api,
                experimental_bridge,
                ..Default::default()
            },
            /*auth_manager*/ None,
        );
        let metadata = test_responses_metadata_for_client(
            &client,
            /*turn_id*/ None,
            "turn:0".into(),
            /*parent_thread_id*/ None,
            TestCodexResponsesRequestKind::Turn,
        );
        let result = client
            .new_session()
            .stream(
                &Prompt::default(),
                &test_model_info(),
                &test_session_telemetry(),
                /*effort*/ None,
                codex_protocol::config_types::ReasoningSummary::None,
                /*service_tier*/ None,
                &metadata,
                &InferenceTraceContext::disabled(),
            )
            .await;
        let Err(error) = result else {
            panic!("an unavailable bridge must fail before any native request");
        };
        let CodexErrorDetails::Fatal(message) = error.details() else {
            panic!("expected a fatal configuration error, got {error}");
        };
        assert_eq!(
            message,
            "The selected model provider requires a model bridge; enable `rust-rig` or \
             `rust-genai`, or select `experimental_bridge = \"native\"` for a Responses provider"
        );
    }
    assert!(
        server
            .received_requests()
            .await
            .expect("captured requests")
            .is_empty()
    );
}
