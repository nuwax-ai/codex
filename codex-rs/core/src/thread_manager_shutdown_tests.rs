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

#[tokio::test]
async fn terminal_drain_stops_an_internal_thread_registered_during_reusable_shutdown() {
    struct BlockFirstStop {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        first_stop: std::sync::atomic::AtomicBool,
    }
    impl codex_extension_api::ThreadLifecycleContributor<Config> for BlockFirstStop {
        fn on_thread_stop<'a>(
            &'a self,
            _input: codex_extension_api::ThreadStopInput<'a>,
        ) -> codex_extension_api::ExtensionFuture<'a, ()> {
            Box::pin(async move {
                if !self.first_stop.swap(true, Ordering::AcqRel) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
            })
        }
    }

    let home = tempdir().expect("temporary home");
    let mut config = test_config().await;
    config.codex_home = home.path().abs();
    config.cwd = home.path().abs();
    config.model_catalog =
        Some(codex_models_manager::bundled_models_response().expect("bundled model catalog"));
    let gate = Arc::new(BlockFirstStop {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        first_stop: std::sync::atomic::AtomicBool::new(false),
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
    let root = manager
        .start_thread(StartThreadOptions::new(config.clone()))
        .await
        .expect("start root thread");
    let stopping_manager = Arc::clone(&manager);
    let stopping = tokio::spawn(async move {
        stopping_manager
            .shutdown_all_threads_bounded(Duration::from_secs(10))
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .expect("root shutdown entered its deterministic gate");

    // This registration is legal during a reusable stop, but is absent from
    // its already-captured snapshot. Final process teardown must include it.
    let late = manager
        .start_thread(StartThreadOptions {
            session_source: Some(SessionSource::Internal(
                InternalSessionSource::MemoryConsolidation,
            )),
            ..StartThreadOptions::new(config)
        })
        .await
        .expect("register late internal thread");
    let late_thread = Arc::clone(&late.thread);
    manager.spawn_shutdown_bound_task(async move {
        late_thread.wait_until_terminated().await;
    });
    gate.release.notify_one();
    let report = stopping.await.expect("join reusable stop");
    assert_eq!(report.completed, vec![root.thread_id]);
    assert!(late.thread.is_running());

    assert!(
        manager
            .wait_for_shutdown_bound_tasks_bounded(Duration::from_secs(10))
            .await
    );
    assert_eq!(late.thread.agent_status().await, AgentStatus::Shutdown);
    assert!(manager.state.threads.read().await.is_empty());
}

#[tokio::test]
async fn reusable_stop_allows_resume_before_terminal_drain() {
    let home = tempdir().expect("temporary home");
    let mut config = test_config().await;
    config.codex_home = home.path().abs();
    config.cwd = home.path().abs();
    config.model_catalog =
        Some(codex_models_manager::bundled_models_response().expect("bundled model catalog"));
    let manager = ThreadManager::with_models_provider_and_home_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
        config.codex_home.to_path_buf(),
        Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
    );
    let original = manager
        .start_thread(StartThreadOptions {
            history_mode: Some(ThreadHistoryMode::Legacy),
            ..StartThreadOptions::new(config.clone())
        })
        .await
        .expect("start persistent thread");
    original.thread.ensure_rollout_materialized().await;
    original
        .thread
        .flush_rollout()
        .await
        .expect("flush rollout");
    let rollout = original.thread.rollout_path().expect("rollout path");
    let report = manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
    assert_eq!(report.completed, vec![original.thread_id]);
    assert!(!manager.is_shutting_down());
    let resumed = manager
        .resume_legacy_thread_from_rollout(
            config,
            rollout,
            Arc::clone(&manager.state.auth_manager),
            /*parent_trace*/ None,
            ClientMcpExtensions::default(),
        )
        .await
        .expect("reuse the same manager to resume the stopped thread");
    assert_eq!(resumed.thread_id, original.thread_id);
    assert!(!Arc::ptr_eq(&resumed.thread, &original.thread));

    assert!(
        manager
            .wait_for_shutdown_bound_tasks_bounded(Duration::from_secs(10))
            .await
    );
    assert!(manager.is_shutting_down());
    assert_eq!(resumed.thread.agent_status().await, AgentStatus::Shutdown);
    assert!(manager.state.threads.read().await.is_empty());
}

#[tokio::test]
async fn reusable_shutdown_does_not_remove_a_concurrently_resumed_runtime() {
    struct BlockSelectedStop {
        thread_id: std::sync::OnceLock<ThreadId>,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl codex_extension_api::ThreadLifecycleContributor<Config> for BlockSelectedStop {
        fn on_thread_stop<'a>(
            &'a self,
            input: codex_extension_api::ThreadStopInput<'a>,
        ) -> codex_extension_api::ExtensionFuture<'a, ()> {
            Box::pin(async move {
                if self
                    .thread_id
                    .get()
                    .is_some_and(|id| input.thread_store.level_id() == id.to_string())
                {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
            })
        }
    }

    let home = tempdir().expect("temporary home");
    let mut config = test_config().await;
    config.codex_home = home.path().abs();
    config.cwd = home.path().abs();
    config.model_catalog =
        Some(codex_models_manager::bundled_models_response().expect("bundled model catalog"));
    let gate = Arc::new(BlockSelectedStop {
        thread_id: std::sync::OnceLock::new(),
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
    let original = manager
        .start_thread(StartThreadOptions {
            history_mode: Some(ThreadHistoryMode::Legacy),
            ..StartThreadOptions::new(config.clone())
        })
        .await
        .expect("start resumable thread A");
    original.thread.ensure_rollout_materialized().await;
    original
        .thread
        .flush_rollout()
        .await
        .expect("flush A rollout");
    let rollout = original.thread.rollout_path().expect("A rollout path");
    let blocking = manager
        .start_thread(StartThreadOptions::new(config.clone()))
        .await
        .expect("start blocking thread B");
    gate.thread_id.set(blocking.thread_id).expect("set B gate");
    let stopping_manager = Arc::clone(&manager);
    let stopping = tokio::spawn(async move {
        stopping_manager
            .shutdown_all_threads_bounded(Duration::from_secs(10))
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .expect("B shutdown entered its deterministic gate");
    original.thread.wait_until_terminated().await;

    // A's old loop is gone, while B prevents the first shutdown from retiring
    // its snapshot entries. Resume A under the same ID with a fresh runtime.
    let resumed = manager
        .resume_legacy_thread_from_rollout(
            config,
            rollout,
            Arc::clone(&manager.state.auth_manager),
            /*parent_trace*/ None,
            ClientMcpExtensions::default(),
        )
        .await
        .expect("resume A while B is stopping");
    assert_eq!(resumed.thread_id, original.thread_id);
    assert!(!Arc::ptr_eq(&resumed.thread, &original.thread));
    gate.release.notify_one();
    let report = stopping.await.expect("join reusable stop");
    let mut completed = vec![original.thread_id, blocking.thread_id];
    completed.sort_by_key(std::string::ToString::to_string);
    assert_eq!(
        report,
        ThreadShutdownReport {
            completed,
            submit_failed: Vec::new(),
            timed_out: Vec::new(),
        }
    );
    assert!(Arc::ptr_eq(
        &manager
            .get_thread(resumed.thread_id)
            .await
            .expect("resumed A remains registered"),
        &resumed.thread,
    ));
    assert!(resumed.thread.is_running());

    assert!(
        manager
            .wait_for_shutdown_bound_tasks_bounded(Duration::from_secs(10))
            .await
    );
    assert_eq!(resumed.thread.agent_status().await, AgentStatus::Shutdown);
    assert!(manager.state.threads.read().await.is_empty());
}
