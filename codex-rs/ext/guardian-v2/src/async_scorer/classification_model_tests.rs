use super::*;
use codex_model_provider_info::ChatBridge;
use codex_protocol::protocol::ThreadSettingsOverrides;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classifier_request_uses_the_captured_active_model_policy() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let thread_server = responses::start_mock_server().await;
    let test = test_codex()
        .with_model_info_override("gpt-6.1-sol", |model| {
            model
                .model_messages
                .as_mut()
                .unwrap()
                .auto_review
                .as_mut()
                .unwrap()
                .policy = Some(TEST_CATALOG_GUARDIAN_POLICY.to_owned());
        })
        .with_model("gpt-5.6-luna")
        .with_config(|config| {
            config.model_provider_id = "native-policy-test".to_string();
            config.model_provider.provider_id = Some("native-policy-test".to_string());
            config.model_provider.experimental_bridge = Some(ChatBridge::Native);
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
            config.features.enable(Feature::GuardianApproval).unwrap();
        })
        .with_pre_build_hook(|home| {
            std::fs::write(home.join("config.toml"), "[features.guardianv2]\nenabled = true\n[features.guardianv2.review_scope]\ncomputer_use_only = false\n").unwrap();
        })
        .build_with_auto_env(&thread_server)
        .await?;
    core_test_support::submit_thread_settings(
        &test.codex,
        ThreadSettingsOverrides {
            model: Some("gpt-6.1-sol".to_string()),
            ..Default::default()
        },
    )
    .await?;
    assert_ne!(
        test.codex.config().await.model.as_deref(),
        Some(test.codex.config_snapshot().await.model.as_str()),
    );
    let thread_store = test.codex.thread_extension_data();
    let active_model = test
        .thread_manager
        .get_models_manager()
        .get_model_info("gpt-6.1-sol", &test.config.to_models_manager_config())
        .await;
    // Tool lifecycle observes metadata captured for the executing step.
    thread_store.insert(active_model);
    let mut connections = vec![Vec::new(); INITIAL_WEBSOCKET_CONNECTIONS - 1];
    connections.push(vec![vec![
        ev_assistant_message("sample-active-policy", "high"),
        ev_completed("response-active-policy"),
    ]]);
    let server = responses::start_websocket_server(connections).await;
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo::create_openai_provider(Some(format!(
        "http://{}/v1",
        server.uri().trim_start_matches("ws://")
    )));
    config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(
        &mut builder,
        AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key")),
        Arc::downgrade(&test.thread_manager),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-policy");
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &session_store,
            thread_store,
        })
        .await;
    thread_store
        .get::<LunaSampler>()
        .unwrap()
        .wait_for_prewarm(PREWARM_TIMEOUT)
        .await?;
    let turn_store = ExtensionData::new("turn-policy");
    registry.tool_lifecycle_contributors()[0]
        .on_tool_start(ToolStartInput {
            permissions: Box::pin(async { Some(Default::default()) }),
            session_store: &session_store,
            thread_store,
            turn_store: &turn_store,
            turn_id: "turn-policy",
            root_turn_id: None,
            call_id: "call-policy",
            originating_item_id: None,
            tool_name: &ToolName::plain("read_file"),
            mcp_tool: None,
            payload: &ToolPayload::Function {
                arguments: "{}".to_string(),
            },
            conversation_history: test.codex.conversation_history_snapshot().await,
            source: ToolCallSource::Direct,
        })
        .await;
    let request = tokio::time::timeout(
        ASYNC_TEST_TIMEOUT,
        server.wait_for_request(
            /*connection_index*/ INITIAL_WEBSOCKET_CONNECTIONS - 1,
            /*request_index*/ 0,
        ),
    )
    .await?;
    assert!(
        request
            .body_json()
            .to_string()
            .contains(TEST_CATALOG_GUARDIAN_POLICY)
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
