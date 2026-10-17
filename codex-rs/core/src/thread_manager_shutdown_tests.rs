use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn shutdown_fences_an_internal_start_before_registry_publication() {
    struct BlockStartup {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl codex_extension_api::ThreadLifecycleContributor<Config> for BlockStartup {
        fn on_thread_start<'a>(
            &'a self,
            _input: codex_extension_api::ThreadStartInput<'a, Config>,
        ) -> codex_extension_api::ExtensionFuture<'a, ()> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
            })
        }
    }

    let home = tempdir().expect("temporary home");
    let mut config = test_config().await;
    config.codex_home = home.path().abs();
    config.cwd = home.path().abs();
    config.model_catalog =
        Some(codex_models_manager::bundled_models_response().expect("bundled model catalog"));
    let gate = Arc::new(BlockStartup {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let mut extensions = codex_extension_api::ExtensionRegistryBuilder::new();
    extensions.thread_lifecycle_contributor(gate.clone());
    let mut manager = ThreadManager::with_models_provider_and_home_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
        config.codex_home.to_path_buf(),
        Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
    );
    Arc::get_mut(&mut manager.state)
        .expect("unshared manager state")
        .extensions = Arc::new(extensions.build());
    let manager = Arc::new(manager);
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let starting_manager = Arc::clone(&manager);
    manager.spawn_shutdown_bound_task(async move {
        let result = starting_manager
            .start_thread(StartThreadOptions {
                session_source: Some(SessionSource::Internal(
                    InternalSessionSource::MemoryConsolidation,
                )),
                ..StartThreadOptions::new(config)
            })
            .await;
        let _ = result_tx.send(result);
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .expect("internal startup entered the deterministic gate");

    let report = manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
    assert_eq!(report, ThreadShutdownReport::default());
    gate.release.notify_one();
    assert!(
        manager
            .wait_for_shutdown_bound_tasks_bounded(Duration::from_secs(10))
            .await
    );
    let result = result_rx.await.expect("startup result");
    assert!(matches!(result, Err(error) if matches!(
        error.details(),
        codex_protocol::error::CodexErrorDetails::InvalidRequest(message)
            if message == "thread manager is shutting down"
    )));
    assert!(manager.state.threads.read().await.is_empty());

    // Shutdown-bound work registered after the fence is dropped, never detached.
    let (late_tx, late_rx) = tokio::sync::oneshot::channel();
    manager.spawn_shutdown_bound_task(async move {
        let _ = late_tx.send(());
    });
    assert!(late_rx.await.is_err());
}
