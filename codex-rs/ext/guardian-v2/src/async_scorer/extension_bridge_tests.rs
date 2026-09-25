use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_provider_does_not_start_native_luna_sampler() -> Result<()> {
    let server = responses::start_mock_server().await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let auth = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key"));
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo {
        name: "Chat gateway".into(),
        provider_id: Some("chat-gateway".into()),
        base_url: Some(format!("{}/v1", server.uri())),
        wire_api: codex_model_provider_info::WireApi::Chat,
        ..ModelProviderInfo::default()
    };
    config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(&mut builder, auth, Arc::downgrade(&test.thread_manager));
    let registry = builder.build();
    let session_store = ExtensionData::new("session-bridge");
    let thread_store = test.codex.thread_extension_data();
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
    assert!(thread_store.get::<LunaSampler>().is_none());
    assert!(
        thread_store
            .get::<codex_extension_api::GuardianV2Enabled>()
            .is_none()
    );
    assert!(
        thread_store
            .get::<codex_core::context::GuardianReviewEvidence>()
            .is_some()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    Ok(())
}
