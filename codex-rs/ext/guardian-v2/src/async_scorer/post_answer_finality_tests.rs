//! Post-answer sampling finality: every gated tool start either sends exactly one
//! bounded Luna request that reaches a definite terminal outcome, or sends none.
//! Covers the trigger chain documented in
//! my-docs/validation-2026-10-07/r1b-delivery-strip-point-analysis-2026-10-10.md
//! 附B: sampler-missing early exits, parent-compaction early exits, retry
//! exhaustion, superseded generations, and pool-pressure cancellation.

use super::*;
use pretty_assertions::assert_eq;

/// Mirrors `sampler::MAX_CONCURRENT_REQUESTS`; the eviction bound under test.
const SAMPLER_MAX_CONCURRENT_REQUESTS: usize = 16;
/// Long enough that delayed classifications stay in flight for the whole test.
const IN_FLIGHT_DELAY: Duration = Duration::from_secs(60);
/// Long enough to inject an authorization change after the request is sent.
const AUTH_CHANGE_DELAY: Duration = Duration::from_secs(3);

fn websocket_request_count(server: &responses::WebSocketTestServer) -> usize {
    server.connections().iter().map(Vec::len).sum()
}

/// (outcome, failure_reason) pairs recorded for the classification metric.
fn recorded_classification_outcomes(metrics: &RecordingMetrics) -> Vec<(String, Option<String>)> {
    metrics
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|sample| match sample {
            RecordedMetric::Counter(name, _, tags) if name == CLASSIFICATION_METRIC => {
                let outcome = tags
                    .iter()
                    .find(|(key, _)| key == "outcome")
                    .map(|(_, value)| value.clone())?;
                let failure_reason = tags
                    .iter()
                    .find(|(key, _)| key == "failure_reason")
                    .map(|(_, value)| value.clone());
                Some((outcome, failure_reason))
            }
            _ => None,
        })
        .collect()
}

fn usable_parent_checkpoint() -> ResponseItem {
    ResponseItem::ContextCompaction {
        id: Some(ResponseItemId::from_server("cmp_parent".to_owned())),
        encrypted_content: Some("encrypted parent summary".to_owned()),
        internal_chat_message_metadata_passthrough: None,
    }
}

/// A thread-owned snapshot whose live checkpoint carries the given producer hash.
fn thread_owned_history(
    checkpoint: ResponseItem,
    compaction_model_hash: Option<String>,
) -> Arc<dyn ConversationHistorySnapshot> {
    Arc::new(TestRetainedHistory {
        current: TestConversationHistory(vec![
            checkpoint,
            user_instruction("Inspect the repository guidelines."),
        ]),
        retained: Vec::new(),
        compaction_model_hash,
        retained_context: Some(codex_history::RetainedContext::default()),
        review_context_revision: 0,
    })
}

struct FinalityFixture {
    test: TestCodex,
    registry: ExtensionRegistry<Config>,
    session_store: ExtensionData,
    metrics: Arc<RecordingMetrics>,
}

async fn start_scored_tool(
    fixture: &FinalityFixture,
    call_id: &str,
    history: Arc<dyn ConversationHistorySnapshot>,
) {
    let thread_store = fixture.test.codex.thread_extension_data();
    let turn_store = ExtensionData::new("turn-1");
    let tool_name = ToolName::plain("read_file");
    let payload = ToolPayload::Function {
        arguments: r#"{"path":"README.md"}"#.to_owned(),
    };
    fixture.registry.tool_lifecycle_contributors()[0]
        .on_tool_start(ToolStartInput {
            permissions: Box::pin(async { Some(Default::default()) }),
            session_store: &fixture.session_store,
            thread_store,
            turn_store: &turn_store,
            turn_id: "turn-1",
            root_turn_id: Some("root-turn"),
            call_id,
            originating_item_id: None,
            tool_name: &tool_name,
            mcp_tool: None,
            payload: &payload,
            conversation_history: history,
            source: ToolCallSource::Direct,
        })
        .await;
}

async fn live_history(fixture: &FinalityFixture) -> Arc<dyn ConversationHistorySnapshot> {
    fixture.test.codex.conversation_history_snapshot().await
}

async fn wait_for_outcome(
    metrics: &RecordingMetrics,
    expected: &[&str],
) -> Result<Vec<(String, Option<String>)>> {
    let outcomes = tokio::time::timeout(ASYNC_TEST_TIMEOUT, async {
        loop {
            let outcomes = recorded_classification_outcomes(metrics);
            if expected
                .iter()
                .all(|outcome| outcomes.iter().any(|(recorded, _)| recorded == outcome))
            {
                return outcomes;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(outcomes)
}

/// Builds a guardian fixture whose Luna pool points at websocket `server`.
/// The scripted responses exist only to detect a request that must not be sent.
async fn unsampled_websocket_fixture(
    server: &responses::WebSocketTestServer,
) -> Result<FinalityFixture> {
    let thread_server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(move |config| {
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
        })
        .with_pre_build_hook(|home| {
            std::fs::write(
                home.join("config.toml"),
                "[features.guardianv2]\nenabled = true\nmax_parent_compaction_tokens = 256\n\n[features.guardianv2.review_scope]\ncomputer_use_only = false\n",
            )
            .expect("Guardian v2 configuration should be written");
        })
        .build_with_auto_env(&thread_server)
        .await?;
    let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key"));
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo::create_openai_provider(Some(format!(
        "http://{}/v1",
        server.uri().trim_start_matches("ws://")
    )));
    config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(
        &mut builder,
        auth_manager,
        Arc::downgrade(&test.thread_manager),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-1");
    let thread_store = test.codex.thread_extension_data();
    thread_store.insert(RecordingMetrics::default());
    let metrics = thread_store.get::<RecordingMetrics>().unwrap();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: Some(metrics.clone()),
            session_store: &session_store,
            thread_store,
        })
        .await;
    thread_store
        .get::<LunaSampler>()
        .expect("Guardian v2 should initialize")
        .wait_for_prewarm(PREWARM_TIMEOUT)
        .await?;
    Ok(FinalityFixture {
        test,
        registry,
        session_store,
        metrics,
    })
}

/// Cell (b): a tool start without a live sampler (never started, or removed by a
/// bridged provider) must send no request and leave no score state behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_start_without_a_live_sampler_sends_no_luna_request() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let thread_server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(move |config| {
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
        })
        .build_with_auto_env(&thread_server)
        .await?;
    let connections = vec![Vec::new(); INITIAL_WEBSOCKET_CONNECTIONS];
    let server = responses::start_websocket_server(connections).await;
    let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key"));
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo::create_openai_provider(Some(format!(
        "http://{}/v1",
        server.uri().trim_start_matches("ws://")
    )));
    config.features.enable(Feature::GuardianV2)?;
    let mut bridge_config = test.config.clone();
    bridge_config.model_provider = ModelProviderInfo {
        name: "Chat gateway".into(),
        provider_id: Some("chat-gateway".into()),
        base_url: Some(format!("{}/v1", server.uri())),
        wire_api: codex_model_provider_info::WireApi::Chat,
        ..ModelProviderInfo::default()
    };
    bridge_config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(
        &mut builder,
        auth_manager,
        Arc::downgrade(&test.thread_manager),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-1");

    // Phase A: the scorer never started, so the tool start exits before observing.
    let fixture = FinalityFixture {
        test,
        registry,
        session_store,
        metrics: Arc::new(RecordingMetrics::default()),
    };
    start_scored_tool(&fixture, "call-1", live_history(&fixture).await).await;
    let thread_store = fixture.test.codex.thread_extension_data();
    assert_eq!(websocket_request_count(&server), 0);
    assert!(server.handshakes().is_empty());
    assert!(thread_store.get::<GuardianV2ScoreProgress>().is_none());
    assert!(
        thread_store
            .get::<codex_extension_api::GuardianV2Enabled>()
            .is_none()
    );
    assert!(cached_score(thread_store).is_none());

    // Phase B: a native provider start installs the sampler...
    fixture.registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &fixture.session_store,
            thread_store,
        })
        .await;
    thread_store
        .get::<LunaSampler>()
        .expect("a native provider should install the sampler");

    // Phase C: ...and a bridged provider start removes it again before any tool runs.
    fixture.registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &bridge_config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &fixture.session_store,
            thread_store,
        })
        .await;
    assert!(thread_store.get::<LunaSampler>().is_none());
    assert!(thread_store.get::<GuardianV2ScoreProgress>().is_none());
    start_scored_tool(&fixture, "call-2", live_history(&fixture).await).await;
    assert_eq!(websocket_request_count(&server), 0);
    assert!(thread_store.get::<GuardianV2ScoreProgress>().is_none());
    assert!(
        thread_store
            .get::<codex_extension_api::GuardianV2Enabled>()
            .is_none()
    );
    assert!(cached_score(thread_store).is_none());
    assert!(recorded_classification_outcomes(&fixture.metrics).is_empty());
    Ok(())
}

/// Cell (c), RequiresSync: a thread-owned checkpoint produced by an incompatible
/// model defers to synchronous review without sending any Luna request and
/// without waiting for an async rescue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_parent_checkpoint_requires_sync_without_sampling() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let connections = vec![Vec::new(); INITIAL_WEBSOCKET_CONNECTIONS];
    let server = responses::start_websocket_server(connections).await;
    let fixture = unsampled_websocket_fixture(&server).await?;
    start_scored_tool(
        &fixture,
        "call-1",
        thread_owned_history(
            usable_parent_checkpoint(),
            Some("parent-model-hash".to_owned()),
        ),
    )
    .await;
    let thread_store = fixture.test.codex.thread_extension_data();

    // The early exit is synchronous: the terminal state exists once the hook returns.
    let fail_closed = cached_score(thread_store)
        .expect("an incompatible checkpoint must fail closed immediately");
    assert_eq!(
        &fail_closed,
        &SecurityRiskScore {
            scores: BTreeMap::from([("action_risk".to_owned(), 1.0)]),
            call_id: None,
            action: None,
            sampled_at: fail_closed.sampled_at,
        }
    );
    assert_eq!(websocket_request_count(&server), 0);
    let progress = thread_store
        .get::<GuardianV2ScoreProgress>()
        .expect("score progress");
    assert!(
        progress.inspect(/*call_id*/ None).has_unscored_failure,
        "RequiresSync must invalidate the observation instead of waiting"
    );
    assert_eq!(
        recorded_classification_outcomes(&fixture.metrics),
        vec![("skipped".to_owned(), None)]
    );
    assert_eq!(
        cached_approval(
            &fixture.registry,
            thread_store,
            "review action",
            /*metrics*/ None,
        )
        .await,
        None
    );
    Ok(())
}

/// Cell (c), Unusable: a malformed live checkpoint fails closed without sampling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unusable_parent_checkpoint_fails_closed_without_sampling() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let connections = vec![Vec::new(); INITIAL_WEBSOCKET_CONNECTIONS];
    let server = responses::start_websocket_server(connections).await;
    let fixture = unsampled_websocket_fixture(&server).await?;
    // The producer hash must match Luna so selection reaches the usability check.
    let mut model_config = fixture.test.config.to_models_manager_config();
    model_config.model_context_window = None;
    let luna_model = fixture
        .test
        .thread_manager
        .get_models_manager()
        .get_model_info(MODEL, &model_config)
        .await;
    let luna_hash = luna_model
        .comp_hash
        .clone()
        .expect("catalog Luna model should declare a compaction hash");
    let unusable = ResponseItem::ContextCompaction {
        id: Some(ResponseItemId::from_server("cmp_unusable".to_owned())),
        encrypted_content: None,
        internal_chat_message_metadata_passthrough: None,
    };
    start_scored_tool(
        &fixture,
        "call-1",
        thread_owned_history(unusable, Some(luna_hash)),
    )
    .await;
    let thread_store = fixture.test.codex.thread_extension_data();

    let fail_closed =
        cached_score(thread_store).expect("an unusable checkpoint must fail closed immediately");
    assert_eq!(
        &fail_closed,
        &SecurityRiskScore {
            scores: BTreeMap::from([("action_risk".to_owned(), 1.0)]),
            call_id: None,
            action: None,
            sampled_at: fail_closed.sampled_at,
        }
    );
    assert_eq!(websocket_request_count(&server), 0);
    let progress = thread_store
        .get::<GuardianV2ScoreProgress>()
        .expect("score progress");
    assert!(
        !progress.inspect(/*call_id*/ None).has_unscored_failure,
        "an unusable checkpoint fails closed without invalidating coverage"
    );
    assert_eq!(
        recorded_classification_outcomes(&fixture.metrics),
        vec![(
            "failure".to_owned(),
            Some("parent_compaction_error".to_owned())
        )]
    );
    assert_eq!(
        cached_approval(
            &fixture.registry,
            thread_store,
            "review action",
            /*metrics*/ None,
        )
        .await,
        None
    );
    Ok(())
}

/// Cell (h): a generation replaced while the Luna response is in flight must end
/// as Superseded without publishing, failing closed, or losing the outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorization_change_supersedes_in_flight_classification_without_publishing() -> Result<()>
{
    skip_if_no_network!(Ok(()));

    let thread_server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(move |config| {
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
            config.guardian_policy_config = Some(TEST_GUARDIAN_POLICY.to_owned());
        })
        .with_pre_build_hook(|home| {
            std::fs::write(
                home.join("config.toml"),
                "[features.guardianv2]\nenabled = true\n\n[features.guardianv2.review_scope]\ncomputer_use_only = false\n",
            )
            .expect("Guardian v2 configuration should be written");
        })
        .build_with_auto_env(&thread_server)
        .await?;
    let http = responses::start_mock_server().await;
    let prompt = |id: &str| responses::sse(vec![ev_assistant_message(id, "low"), ev_completed(id)]);
    let classified = responses::mount_response_sequence(
        &http,
        vec![
            responses::sse_response(prompt("first")),
            responses::sse_response(prompt("second")).set_delay(AUTH_CHANGE_DELAY),
        ],
    )
    .await;
    let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key"));
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo::create_openai_provider(Some(http.uri()));
    config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(
        &mut builder,
        auth_manager,
        Arc::downgrade(&test.thread_manager),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-1");
    let thread_store = test.codex.thread_extension_data();
    thread_store.insert(RecordingMetrics::default());
    let metrics = thread_store.get::<RecordingMetrics>().unwrap();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: Some(metrics.clone()),
            session_store: &session_store,
            thread_store,
        })
        .await;
    let fixture = FinalityFixture {
        test,
        registry,
        session_store,
        metrics,
    };
    let thread_store = fixture.test.codex.thread_extension_data();

    // A legitimate request is sent and reaches a definite terminal outcome.
    start_scored_tool(&fixture, "call-1", live_history(&fixture).await).await;
    tokio::time::timeout(ASYNC_TEST_TIMEOUT, async {
        loop {
            if let Some(score) = cached_score(thread_store)
                && score.call_id.as_deref() == Some("call-1")
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(classified.requests().len(), 1);

    // Replace the generation while the second response is deliberately in flight.
    start_scored_tool(&fixture, "call-2", live_history(&fixture).await).await;
    tokio::time::timeout(ASYNC_TEST_TIMEOUT, async {
        while classified.requests().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    fixture
        .test
        .codex
        .inject_response_items(vec![ResponseItem::Message {
            id: None,
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: "Stop. Do not change any files.".to_owned(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }])
        .await?;
    let outcomes = wait_for_outcome(&fixture.metrics, &["success", "superseded"]).await?;
    assert_eq!(
        outcomes,
        vec![
            ("success".to_owned(), None),
            ("superseded".to_owned(), None),
        ]
    );
    // The superseded generation publishes nothing and fails closed on nothing:
    // the only published score is still the first call's.
    let score = cached_score(thread_store).expect("the first sample stays published");
    assert_eq!(score.call_id.as_deref(), Some("call-1"));
    assert_eq!(score.scores.get("action_risk"), Some(&0.0));
    assert_eq!(
        fixture
            .metrics
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|sample| matches!(sample, RecordedMetric::Counter(name, _, _) if name == CLASSIFICATION_RISK_METRIC))
            .count(),
        1,
        "only the first classification may record a risk level"
    );
    let progress = thread_store
        .get::<GuardianV2ScoreProgress>()
        .expect("score progress");
    assert!(!progress.inspect(/*call_id*/ None).has_unscored_failure);
    Ok(())
}

/// Cell (e): exhausting the bounded transport retries through the contributor
/// fails closed after exactly MAX_SAMPLING_RETRIES + 1 attempts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_transport_retries_fail_closed_after_bounded_attempts() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let thread_server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(move |config| {
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
            config.guardian_policy_config = Some(TEST_GUARDIAN_POLICY.to_owned());
        })
        .with_pre_build_hook(|home| {
            std::fs::write(
                home.join("config.toml"),
                "[features.guardianv2]\nenabled = true\n\n[features.guardianv2.review_scope]\ncomputer_use_only = false\n",
            )
            .expect("Guardian v2 configuration should be written");
        })
        .build_with_auto_env(&thread_server)
        .await?;
    let unavailable = || {
        vec![vec![vec![json!({
            "type": "error",
            "status": 503,
            "error": {
                "type": "server_error",
                "message": "temporarily unavailable"
            }
        })]]]
    };
    let first = responses::start_websocket_server(unavailable()).await;
    let second = responses::start_websocket_server(vec![vec![vec![json!({
        "type": "response.failed",
        "response": {"error": {"code": "flex_unavailable", "message": "capacity unavailable"}}
    })]]])
    .await;
    let http = responses::start_mock_server().await;
    let http_failure = responses::mount_sse_once(
        &http,
        responses::sse(vec![json!({
            "type": "response.failed", "response": {
                "error": {"code": "flex_unavailable", "message": "HTTP sampling failed"}
            }
        })]),
    )
    .await;
    let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key"));
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo::create_openai_provider(Some(
        proxy_websocket_servers_with_http(
            &[&first, &second],
            ProxyPrewarmLimit::AllConnections,
            Some(&http.uri()),
        )
        .await?,
    ));
    config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(
        &mut builder,
        auth_manager,
        Arc::downgrade(&test.thread_manager),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-1");
    let thread_store = test.codex.thread_extension_data();
    thread_store.insert(RecordingMetrics::default());
    let metrics = thread_store.get::<RecordingMetrics>().unwrap();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: Some(metrics.clone()),
            session_store: &session_store,
            thread_store,
        })
        .await;
    thread_store
        .get::<LunaSampler>()
        .expect("Guardian v2 should initialize")
        .wait_for_prewarm(PREWARM_TIMEOUT)
        .await?;
    let fixture = FinalityFixture {
        test,
        registry,
        session_store,
        metrics,
    };
    let thread_store = fixture.test.codex.thread_extension_data();

    start_scored_tool(&fixture, "call-1", live_history(&fixture).await).await;
    tokio::time::timeout(ASYNC_TEST_TIMEOUT, async {
        loop {
            if let Some(score) = cached_score(thread_store)
                && score.scores.get("action_risk") == Some(&1.0)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    // The failure is terminal after the bounded attempts, with no extra waiting.
    assert_eq!(
        recorded_classification_outcomes(&fixture.metrics),
        vec![("failure".to_owned(), Some("flex_unavailable".to_owned()))]
    );
    assert_eq!(
        websocket_request_count(&first) + websocket_request_count(&second),
        2,
        "each warm socket is consumed exactly once"
    );
    assert_eq!(http_failure.requests().len(), 1);
    assert_eq!(
        cached_approval(
            &fixture.registry,
            thread_store,
            "review action",
            /*metrics*/ None,
        )
        .await,
        None
    );
    Ok(())
}

/// Cell (i): pool pressure cancels the oldest unfinished classification with a
/// definite Superseded outcome while the newest request still completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_pressure_cancels_the_oldest_classification_with_a_terminal_outcome() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let thread_server = responses::start_mock_server().await;
    let test = test_codex()
        .with_config(move |config| {
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
            config.guardian_policy_config = Some(TEST_GUARDIAN_POLICY.to_owned());
        })
        .with_pre_build_hook(|home| {
            std::fs::write(
                home.join("config.toml"),
                "[features.guardianv2]\nenabled = true\n\n[features.guardianv2.review_scope]\ncomputer_use_only = false\n",
            )
            .expect("Guardian v2 configuration should be written");
        })
        .build_with_auto_env(&thread_server)
        .await?;
    let http = responses::start_mock_server().await;
    let stalled = |id: &str| {
        responses::sse_response(responses::sse(vec![
            ev_assistant_message(id, "low"),
            ev_completed(id),
        ]))
        .set_delay(IN_FLIGHT_DELAY)
    };
    let mut responses_sequence = (0..SAMPLER_MAX_CONCURRENT_REQUESTS)
        .map(|index| stalled(&format!("stalled-{index}")))
        .collect::<Vec<_>>();
    responses_sequence.push(responses::sse_response(responses::sse(vec![
        ev_assistant_message("replacement", "low"),
        ev_completed("replacement"),
    ])));
    let requests = responses::mount_response_sequence(&http, responses_sequence).await;
    let auth_manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-api-key"));
    let mut config = test.config.clone();
    config.model_provider = ModelProviderInfo::create_openai_provider(Some(http.uri()));
    config.features.enable(Feature::GuardianV2)?;
    let mut builder = ExtensionRegistryBuilder::new();
    super::super::install(
        &mut builder,
        auth_manager,
        Arc::downgrade(&test.thread_manager),
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session-1");
    let thread_store = test.codex.thread_extension_data();
    thread_store.insert(RecordingMetrics::default());
    let metrics = thread_store.get::<RecordingMetrics>().unwrap();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Exec,
            persistent_thread_state_available: false,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: Some(metrics.clone()),
            session_store: &session_store,
            thread_store,
        })
        .await;
    let fixture = FinalityFixture {
        test,
        registry,
        session_store,
        metrics,
    };
    let thread_store = fixture.test.codex.thread_extension_data();

    // Fill the sampler's concurrency budget with unfinished classifications.
    let history = live_history(&fixture).await;
    for index in 0..SAMPLER_MAX_CONCURRENT_REQUESTS {
        start_scored_tool(&fixture, &format!("call-{index}"), history.clone()).await;
    }
    tokio::time::timeout(ASYNC_TEST_TIMEOUT, async {
        while requests.requests().len() < SAMPLER_MAX_CONCURRENT_REQUESTS {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    // The next classification must evict the oldest one instead of queueing.
    start_scored_tool(&fixture, "call-replacement", history).await;
    tokio::time::timeout(ASYNC_TEST_TIMEOUT, async {
        while requests.requests().len() < SAMPLER_MAX_CONCURRENT_REQUESTS + 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let outcomes = wait_for_outcome(&fixture.metrics, &["superseded", "success"]).await?;
    assert_eq!(
        outcomes,
        vec![
            ("superseded".to_owned(), None),
            ("success".to_owned(), None),
        ],
        "the cancelled classification keeps a definite terminal outcome"
    );
    let score = cached_score(thread_store).expect("the replacement publishes its score");
    assert_eq!(score.scores.get("action_risk"), Some(&0.0));
    assert!(score.call_id.is_some());
    let progress = thread_store
        .get::<GuardianV2ScoreProgress>()
        .expect("score progress");
    assert!(!progress.inspect(/*call_id*/ None).has_unscored_failure);
    assert_eq!(
        requests.requests().len(),
        SAMPLER_MAX_CONCURRENT_REQUESTS + 1
    );
    Ok(())
}
