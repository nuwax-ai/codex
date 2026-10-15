//! Captures both memory requests after runtime model selection, through the startup entry point.

use super::*;
use codex_model_provider_info::ChatBridge;
use codex_protocol::protocol::ThreadSettingsOverrides;
use core_test_support::responses::mount_sse_sequence;
use pretty_assertions::assert_eq;

enum ParentModel {
    CatalogDefault,
    RuntimeOverride,
}

enum MemoryModels {
    Active,
    ExplicitOverrides,
}

#[tokio::test]
async fn startup_requests_use_the_catalog_default_when_config_model_is_absent() -> anyhow::Result<()>
{
    capture_startup_models(ParentModel::CatalogDefault, MemoryModels::Active).await
}

#[tokio::test]
async fn startup_requests_follow_the_runtime_model_override() -> anyhow::Result<()> {
    capture_startup_models(ParentModel::RuntimeOverride, MemoryModels::Active).await
}

#[tokio::test]
async fn startup_requests_preserve_explicit_memory_model_overrides() -> anyhow::Result<()> {
    capture_startup_models(
        ParentModel::RuntimeOverride,
        MemoryModels::ExplicitOverrides,
    )
    .await
}

async fn capture_startup_models(
    parent_model: ParentModel,
    memory_models: MemoryModels,
) -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    let mut memories = startup_test_memories_config();
    let expected_overrides = match memory_models {
        MemoryModels::Active => None,
        MemoryModels::ExplicitOverrides => {
            memories.extract_model = Some("gpt-5.6-luna".to_string());
            memories.consolidation_model = Some("gpt-5.6-terra".to_string());
            Some(["gpt-5.6-luna", "gpt-5.6-terra"])
        }
    };
    let catalog = codex_models_manager::bundled_models_response()?;
    let use_catalog_default = matches!(parent_model, ParentModel::CatalogDefault);
    let test = test_codex()
        .with_home(Arc::clone(&home))
        .with_config(move |config| {
            config
                .features
                .enable(Feature::Sqlite)
                .expect("enable sqlite");
            config.memories = memories;
            config.model = if use_catalog_default {
                None
            } else {
                Some("gpt-5.5".to_string())
            };
            config.model_catalog = Some(catalog);
            config.model_provider_id = "memory-model-test".to_string();
            config.model_provider.provider_id = Some("memory-model-test".to_string());
            config.model_provider.name = "Memory model test".to_string();
            config.model_provider.requires_openai_auth = false;
            config.model_provider.experimental_bridge = Some(ChatBridge::Native);
        })
        .build_with_auto_env(&server)
        .await?;
    if matches!(parent_model, ParentModel::RuntimeOverride) {
        core_test_support::submit_thread_settings(
            &test.codex,
            ThreadSettingsOverrides {
                model: Some("gpt-5.6-sol".to_string()),
                ..Default::default()
            },
        )
        .await?;
    }
    let active_model = test.codex.config_snapshot().await.model;
    assert_ne!(
        test.codex.config().await.model.as_deref(),
        Some(active_model.as_str())
    );
    let db = test.codex.state_db().expect("memory state db");
    seed_stage1_candidate(
        db.as_ref(),
        home.path(),
        chrono::Utc::now() - chrono::Duration::hours(2),
        "runtime-model",
    )
    .await?;
    let root = home.path().join("memories");
    seed_required_memory_artifacts(&root).await?;
    let output = json!({
        "raw_memory": "runtime model memory",
        "rollout_summary": "runtime model summary",
        "rollout_slug": "runtime-model",
    });
    // The startup pipeline sends extraction before consolidation; one sequence
    // captures both without a race between installing their mock responses.
    let response = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-model-phase1"),
                ev_assistant_message("msg-model-phase1", &output.to_string()),
                ev_completed("resp-model-phase1"),
            ]),
            sse(vec![
                ev_response_created("resp-model-phase2"),
                ev_assistant_message("msg-model-phase2", "phase2 complete"),
                ev_completed("resp-model-phase2"),
            ]),
        ],
    )
    .await;
    trigger_memories_startup(&test).await;
    let requests = wait_for_request(&response, /*expected_count*/ 2).await;
    let expected = expected_overrides.unwrap_or([active_model.as_str(); 2]);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.body_json()["model"].as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        expected.map(str::to_string).to_vec(),
    );
    wait_for_phase2_workspace_reset(db.memories(), &root).await?;
    shutdown_test_codex(&test).await?;
    Ok(())
}
