//! Verifies observed Responses API usage is durably recorded in rollout history.

use anyhow::Result;
use codex_core::RecoverTurnRequest;
use codex_core::StartIfIdleSubmission;
use codex_core::TurnInputRequest;
use codex_history::RolloutItem;
use codex_protocol::SessionId;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::TokenUsageRecord;
use codex_protocol::turn_input::SuspendTurnOutcome;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_sequence;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn token_usage_records(path: &std::path::Path) -> Vec<TokenUsageRecord> {
    std::fs::read_to_string(path)
        .expect("read rollout")
        .lines()
        .filter_map(|line| codex_rollout::parse_rollout_line(line).ok())
        .filter_map(|line| match line.item {
            RolloutItem::TokenUsageRecord(record) => Some(record),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovered_turn_keeps_prior_sampling_usage_on_a_new_session() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let plan_args =
        json!({"plan": [{"step": "keep sampling", "status": "in_progress"}]}).to_string();
    let requests = mount_response_sequence(
        &server,
        vec![
            sse_response(sse(vec![
                ev_response_created("response-reused"),
                ev_function_call("call-before-recovery", "update_plan", &plan_args),
                ev_completed_with_tokens("response-reused", /*total_tokens*/ 30),
            ])),
            sse_response(sse(vec![ev_completed_with_tokens(
                "delayed", /*total_tokens*/ 100,
            )]))
            .set_delay(Duration::from_secs(/*secs*/ 60)),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let submission = test
        .codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "recover this unfinished turn".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let codex_core::TurnInputSubmission::Started { turn_id, .. } = submission else {
        panic!("expected a started turn");
    };
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TokenCount(event)
        if event.info.as_ref().is_some_and(|info| info.last_token_usage.total_tokens == 30))
    })
    .await;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 10), async {
        while requests.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .expect("second sampling reached the server before suspension");
    assert_eq!(
        test.codex.suspend_turn_and_shutdown().await?,
        SuspendTurnOutcome::Suspended {
            turn_id: turn_id.clone(),
        }
    );
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    test.thread_manager
        .remove_thread(&test.session_configured.thread_id)
        .await
        .expect("remove suspended thread");

    let recovery_server = start_mock_server().await;
    mount_sse_once(
        &recovery_server,
        sse(vec![
            ev_response_created("response-reused"),
            ev_assistant_message("recovered-message", "done"),
            ev_completed_with_tokens("response-reused", /*total_tokens*/ 40),
        ]),
    )
    .await;
    let cwd = test.config.cwd.clone();
    let mut resume_builder = test_codex().with_config(move |config| config.cwd = cwd);
    if let Some(url) = test.executor_environment().exec_server_url() {
        resume_builder = resume_builder.with_exec_server_url(url);
    }
    let resumed = resume_builder
        .resume(
            &recovery_server,
            Arc::clone(&test.home),
            rollout_path.clone(),
        )
        .await?;
    assert_eq!(
        resumed
            .codex
            .recover_turn_if_idle(RecoverTurnRequest {
                turn_id: turn_id.clone(),
                thread_settings: Default::default(),
                trace: None,
                cyber_access_program: None,
            })
            .await?,
        StartIfIdleSubmission::Started {
            root_turn_id: turn_id.clone(),
            turn_id: turn_id.clone()
        }
    );
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    resumed.codex.shutdown_and_wait().await?;

    let records = token_usage_records(&rollout_path);
    assert_eq!(
        records
            .iter()
            .map(|record| (
                record.turn_id.as_str(),
                record.turn_token_usage.total_tokens,
                record.thread_token_usage.total_tokens,
            ))
            .collect::<Vec<_>>(),
        vec![(turn_id.as_str(), 30, 30), (turn_id.as_str(), 70, 70)]
    );
    assert!(records.iter().all(|record| record.response_key.is_some()));
    assert_ne!(records[0].response_key, records[1].response_key);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_response_usage_accumulates_per_turn_and_thread() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let plan_args = json!({
        "plan": [{
            "step": "keep sampling",
            "status": "in_progress"
        }]
    })
    .to_string();
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("response-a"),
                ev_function_call("call-a", "update_plan", &plan_args),
                ev_completed_with_tokens("response-a", /*total_tokens*/ 120),
            ]),
            sse(vec![
                ev_response_created("response-b"),
                ev_assistant_message("message-b", "done"),
                ev_completed_with_tokens("response-b", /*total_tokens*/ 80),
            ]),
            sse(vec![
                ev_response_created("response-c"),
                ev_assistant_message("message-c", "next"),
                ev_completed_with_tokens("response-c", /*total_tokens*/ 30),
            ]),
            sse(vec![
                ev_response_created("response-without-usage"),
                ev_assistant_message("message-d", "no usage"),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "response-without-usage"
                    }
                }),
            ]),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let home = test.home.clone();

    test.submit_turn("first").await?;
    test.codex.shutdown_and_wait().await?;

    let resumed = test_codex()
        .resume(&server, home, rollout_path.clone())
        .await?;
    for prompt in ["second", "third"] {
        resumed.submit_turn(prompt).await?;
    }
    resumed.codex.shutdown_and_wait().await?;

    let records = token_usage_records(&rollout_path);
    assert_eq!(records.len(), 3);
    assert_eq!(
        records
            .iter()
            .map(|record| {
                (
                    record.response_id.as_str(),
                    record.turn_token_usage.total_tokens,
                    record.thread_token_usage.total_tokens,
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("response-a", 120, 120),
            ("response-b", 200, 200),
            ("response-c", 30, 230),
        ]
    );
    assert_eq!(records[0].turn_id, records[1].turn_id);
    assert_ne!(records[1].turn_id, records[2].turn_id);
    assert!(records.iter().all(|record| {
        record.session_id == SessionId::from(record.thread_id)
            && record.root_turn_id == record.turn_id
    }));

    Ok(())
}

/// Reusing one provider response id across DISTINCT wire requests must not
/// alias their usage: every sampling joins the ledger by its own host key,
/// live and after resume, so each report counts and nothing replaces another
/// sampling's snapshot. (The replace-under-one-key contract lives in the
/// SessionState unit tests.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_id_responses_across_distinct_samplings_count_separately_after_resume() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let plan_args = json!({"plan":[{"step":"keep sampling","status":"in_progress"}]}).to_string();
    let mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("response-a"),
                ev_assistant_message("a1", "first"),
                ev_completed_with_tokens("response-a", /*total_tokens*/ 30),
            ]),
            sse(vec![
                ev_response_created("response-b"),
                ev_function_call("call-b", "update_plan", &plan_args),
                ev_completed_with_tokens("response-b", /*total_tokens*/ 10),
            ]),
            sse(vec![
                ev_response_created("response-a"),
                ev_assistant_message("a2", "second"),
                ev_completed_with_tokens("response-a", /*total_tokens*/ 45),
            ]),
            sse(vec![
                ev_response_created("response-a"),
                ev_function_call("call-c", "update_plan", &plan_args),
                ev_completed_with_tokens("response-a", /*total_tokens*/ 50),
            ]),
            sse(vec![
                ev_response_created("response-a"),
                ev_assistant_message("a3", "third"),
                ev_completed_with_tokens("response-a", /*total_tokens*/ 55),
            ]),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let home = test.home.clone();
    test.submit_turn("first").await?;
    test.submit_turn("second").await?;
    test.codex.shutdown_and_wait().await?;
    let resumed = test_codex()
        .resume(&server, home, rollout_path.clone())
        .await?;
    resumed.submit_turn("third").await?;
    resumed.codex.shutdown_and_wait().await?;
    let records = token_usage_records(&rollout_path);
    assert_eq!(
        records
            .iter()
            .map(|r| (
                r.response_id.as_str(),
                r.usage.total_tokens,
                r.turn_token_usage.total_tokens,
                r.thread_token_usage.total_tokens,
            ))
            .collect::<Vec<_>>(),
        vec![
            ("response-a", 30, 30, 30),
            ("response-b", 10, 10, 40),
            ("response-a", 45, 55, 85),
            ("response-a", 50, 50, 135),
            ("response-a", 55, 105, 190),
        ]
    );
    // Every turn sampling carries its own host key; provider ids alone never
    // identify a sampling.
    let keys: Vec<&str> = records
        .iter()
        .map(|record| record.response_key.as_deref().unwrap_or_default())
        .collect();
    assert!(
        keys.iter()
            .all(|key| key.starts_with("sampling:") && key.ends_with(|c: char| c.is_ascii_digit())),
        "each sampling persists its host key: {keys:?}"
    );
    assert_eq!(
        keys.iter().collect::<std::collections::HashSet<_>>().len(),
        keys.len(),
        "host keys are unique per sampling even for a repeated provider id"
    );
    assert_eq!(mock.requests().len(), 5);
    Ok(())
}

#[derive(Clone, Copy)]
enum LegacyUsageIdentity {
    BoundedHost,
    OversizedHost,
    Keyless,
}

#[test_case::test_case(LegacyUsageIdentity::BoundedHost; "bounded_host")]
#[test_case::test_case(LegacyUsageIdentity::OversizedHost; "oversized_host")]
#[test_case::test_case(LegacyUsageIdentity::Keyless; "keyless")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_bounds_usage_identities_without_rewriting_history(
    identity: LegacyUsageIdentity,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_assistant_message("before-resume", "first"),
                ev_completed_with_tokens("response-before-resume", /*total_tokens*/ 30),
            ]),
            // No usage is reported by compaction: its checkpoint exposes the
            // restored latest record without replacing it with a new report.
            sse(vec![
                ev_assistant_message("compact-summary", "summary"),
                json!({"type": "response.completed", "response": {"id": "compact"}}),
            ]),
            sse(vec![
                ev_assistant_message("after-resume", "second"),
                ev_completed_with_tokens("response-after-resume", /*total_tokens*/ 40),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| config.model_provider.name = "OpenAI-compatible test provider".into())
        .build_with_auto_env(&server)
        .await?;
    test.submit_turn("first").await?;
    test.codex.flush_rollout().await?;
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let mut legacy = token_usage_records(&rollout_path)
        .pop()
        .expect("initial sampling usage");
    match identity {
        LegacyUsageIdentity::BoundedHost => {
            legacy.response_id = "p".repeat(/*n*/ 1024 * 1024);
        }
        LegacyUsageIdentity::OversizedHost => {
            legacy.response_key = Some("h".repeat(/*n*/ 1024 * 1024));
        }
        LegacyUsageIdentity::Keyless => {
            legacy.response_key = None;
            legacy.response_id = "p".repeat(/*n*/ 1024 * 1024);
        }
    }
    // Append a historical fixture through the public persistence boundary.
    test.codex
        .append_rollout_items(&[RolloutItem::TokenUsageRecord(legacy.clone())])
        .await?;
    test.codex.shutdown_and_wait().await?;
    let original_prefix = std::fs::read(&rollout_path)?;
    let mut expected_restored = legacy.clone();
    expected_restored.response_id.clear();
    if matches!(identity, LegacyUsageIdentity::OversizedHost) {
        expected_restored.response_key = None;
    }

    let resume_builder = || {
        let cwd = test.config.cwd.clone();
        let mut builder = test_codex().with_config(move |config| {
            config.cwd = cwd;
            config.model_provider.name = "OpenAI-compatible test provider".into();
        });
        if let Some(url) = test.executor_environment().exec_server_url() {
            builder = builder.with_exec_server_url(url);
        }
        builder
    };
    let resumed = resume_builder()
        .resume(&server, Arc::clone(&test.home), rollout_path.clone())
        .await?;
    resumed.codex.submit(Op::Compact).await?;
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    resumed.codex.shutdown_and_wait().await?;
    let checkpoint = std::fs::read_to_string(&rollout_path)?
        .lines()
        .filter_map(|line| codex_rollout::parse_rollout_line(line).ok())
        .find_map(|line| match line.item {
            RolloutItem::Compacted(compacted) => compacted.latest_token_usage_record,
            _ => None,
        })
        .expect("compaction checkpoint keeps restored usage");
    assert_eq!(checkpoint, expected_restored);

    let resumed_again = resume_builder()
        .resume(&server, Arc::clone(&test.home), rollout_path.clone())
        .await?;
    resumed_again.submit_turn("second").await?;
    resumed_again.codex.shutdown_and_wait().await?;
    let records = token_usage_records(&rollout_path);
    let latest = records.last().expect("new sampling usage");
    assert_eq!(
        (
            latest.usage.total_tokens,
            latest.thread_token_usage.total_tokens
        ),
        (40, 70)
    );
    assert_eq!(records.len(), 3);
    assert_eq!(records[1], legacy);
    assert!(std::fs::read(&rollout_path)?.starts_with(&original_prefix));
    assert_eq!(mock.requests().len(), 3);
    Ok(())
}
