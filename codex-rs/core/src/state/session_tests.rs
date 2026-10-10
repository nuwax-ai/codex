use super::*;
use crate::session::tests::make_session_configuration_for_tests;
use crate::state::AutoCompactWindowSnapshot;
use codex_protocol::SessionId;
use codex_protocol::ThreadId;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use codex_protocol::protocol::SpendControlLimitSnapshot;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageRecord;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn record_token_usage_continues_restored_totals() {
    let thread_id = ThreadId::new();
    let session_id = SessionId::from(ThreadId::new());
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut restored = SessionState::new(make_session_configuration_for_tests().await);
    restored.latest_token_usage_record = Some(TokenUsageRecord {
        thread_id,
        turn_id: "turn-b".to_string(),
        session_id,
        root_turn_id: "root-turn".to_string(),
        response_id: "response-c".to_string(),
        usage: usage(30),
        turn_token_usage: usage(30),
        thread_token_usage: usage(230),
    });
    let after_resume = restored.record_token_usage(
        thread_id,
        "turn-b",
        session_id,
        "root-turn".to_string(),
        "response-d".to_string(),
        &usage(20),
    );
    assert_eq!(
        after_resume,
        TokenUsageRecord {
            thread_id,
            turn_id: "turn-b".to_string(),
            session_id,
            root_turn_id: "root-turn".to_string(),
            response_id: "response-d".to_string(),
            usage: usage(20),
            turn_token_usage: usage(50),
            thread_token_usage: usage(250),
        }
    );
}

#[tokio::test]
async fn repeated_response_reports_replace_their_snapshot() {
    let thread_id = ThreadId::new();
    let session_id = SessionId::from(ThreadId::new());
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut state = SessionState::new(make_session_configuration_for_tests().await);

    let first = state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        "response-a".to_string(),
        &usage(30),
    );
    assert_eq!(first.turn_token_usage, usage(30));
    assert_eq!(first.thread_token_usage, usage(30));

    // The same response re-reports a LARGER cumulative snapshot (pause
    // continuation / duplicate delivery): the totals swap the old snapshot
    // for the new one instead of adding on top.
    let replaced = state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        "response-a".to_string(),
        &usage(45),
    );
    assert_eq!(replaced.turn_token_usage, usage(45));
    assert_eq!(replaced.thread_token_usage, usage(45));

    // A second response in the same turn still adds.
    let second = state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        "response-b".to_string(),
        &usage(10),
    );
    assert_eq!(second.turn_token_usage, usage(55));
    assert_eq!(second.thread_token_usage, usage(55));
}

#[tokio::test]
async fn keyless_reports_stay_additive() {
    let thread_id = ThreadId::new();
    let session_id = SessionId::from(ThreadId::new());
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut state = SessionState::new(make_session_configuration_for_tests().await);
    // Keyless snapshots cannot replace anything: each report adds.
    for (report, expected_total) in [(10, 10), (25, 35), (25, 60)] {
        let record = state.record_token_usage(
            thread_id,
            "turn-1",
            session_id,
            "root".to_string(),
            String::new(),
            &usage(report),
        );
        assert_eq!(record.turn_token_usage, usage(expected_total));
        assert_eq!(record.thread_token_usage, usage(expected_total));
    }
}

#[tokio::test]
async fn ledger_survives_into_new_turns_without_double_counting() {
    let thread_id = ThreadId::new();
    let session_id = SessionId::from(ThreadId::new());
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut state = SessionState::new(make_session_configuration_for_tests().await);
    state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        "response-a".to_string(),
        &usage(30),
    );
    // A new turn starts a fresh turn subtotal while the thread keeps its
    // total; re-report of turn-1's response must not inflate turn-2.
    let replayed = state.record_token_usage(
        thread_id,
        "turn-2",
        session_id,
        "root".to_string(),
        "response-a".to_string(),
        &usage(45),
    );
    assert_eq!(
        replayed.turn_token_usage,
        usage(45),
        "a replayed response nets its new snapshot into the current turn"
    );
    assert_eq!(
        replayed.thread_token_usage,
        usage(45),
        "the thread total holds the replaced snapshot, not the sum"
    );
}

#[tokio::test]
// Verifies connector merging deduplicates repeated IDs.
async fn merge_connector_selection_deduplicates_entries() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    let merged = state.merge_connector_selection([
        "calendar".to_string(),
        "calendar".to_string(),
        "drive".to_string(),
    ]);

    assert_eq!(
        merged,
        HashSet::from(["calendar".to_string(), "drive".to_string()])
    );
}

#[tokio::test]
// Verifies clearing connector selection removes all saved IDs.
async fn clear_connector_selection_removes_entries() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    state.merge_connector_selection(["calendar".to_string()]);

    state.clear_connector_selection();

    assert_eq!(state.get_connector_selection(), HashSet::new());
}

#[tokio::test]
async fn set_rate_limits_defaults_limit_id_to_codex_when_missing() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    state.set_rate_limits(RateLimitSnapshot {
        limit_id: None,
        limit_name: None,
        normal_model_slug: None,
        primary: Some(RateLimitWindow {
            used_percent: 12.0,
            window_minutes: Some(60),
            resets_at: Some(100),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        spend_control_reached: None,
        plan_type: None,
        rate_limit_reached_type: None,
    });

    assert_eq!(
        state
            .latest_rate_limits
            .as_ref()
            .and_then(|v| v.limit_id.clone()),
        Some("codex".to_string())
    );
}

#[tokio::test]
async fn replace_history_clears_auto_compact_window_prefill() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    state.set_auto_compact_window_estimated_prefill(/*tokens*/ 100);
    state.replace_history(Vec::new(), /*reference_context_item*/ None);

    assert_eq!(
        state.auto_compact_window_snapshot(),
        AutoCompactWindowSnapshot {
            prefill_input_tokens: None,
        }
    );
}

#[tokio::test]
async fn set_rate_limits_defaults_to_codex_when_limit_id_missing_after_other_bucket() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    state.set_rate_limits(RateLimitSnapshot {
        limit_id: Some("codex_other".to_string()),
        limit_name: Some("codex_other".to_string()),
        normal_model_slug: None,
        primary: Some(RateLimitWindow {
            used_percent: 20.0,
            window_minutes: Some(60),
            resets_at: Some(200),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        spend_control_reached: None,
        plan_type: None,
        rate_limit_reached_type: None,
    });
    state.set_rate_limits(RateLimitSnapshot {
        limit_id: None,
        limit_name: None,
        normal_model_slug: None,
        primary: Some(RateLimitWindow {
            used_percent: 30.0,
            window_minutes: Some(60),
            resets_at: Some(300),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        spend_control_reached: None,
        plan_type: None,
        rate_limit_reached_type: None,
    });

    assert_eq!(
        state
            .latest_rate_limits
            .as_ref()
            .and_then(|v| v.limit_id.clone()),
        Some("codex".to_string())
    );
}

#[tokio::test]
async fn set_rate_limits_carries_account_metadata_from_codex_to_codex_other() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    state.set_rate_limits(RateLimitSnapshot {
        limit_id: Some("codex".to_string()),
        limit_name: Some("codex".to_string()),
        normal_model_slug: None,
        primary: Some(RateLimitWindow {
            used_percent: 10.0,
            window_minutes: Some(60),
            resets_at: Some(100),
        }),
        secondary: None,
        credits: Some(CreditsSnapshot {
            has_credits: true,
            unlimited: false,
            balance: Some("50".to_string()),
        }),
        individual_limit: Some(SpendControlLimitSnapshot {
            limit: "25000".to_string(),
            used: "8000".to_string(),
            remaining_percent: 68,
            resets_at: 300,
        }),
        spend_control_reached: Some(true),
        plan_type: Some(codex_protocol::account::PlanType::Plus),
        rate_limit_reached_type: None,
    });

    state.set_rate_limits(RateLimitSnapshot {
        limit_id: Some("codex_other".to_string()),
        limit_name: None,
        normal_model_slug: None,
        primary: Some(RateLimitWindow {
            used_percent: 30.0,
            window_minutes: Some(120),
            resets_at: Some(200),
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        spend_control_reached: None,
        plan_type: None,
        rate_limit_reached_type: None,
    });

    assert_eq!(
        state.latest_rate_limits,
        Some(RateLimitSnapshot {
            limit_id: Some("codex_other".to_string()),
            limit_name: None,
            normal_model_slug: None,
            primary: Some(RateLimitWindow {
                used_percent: 30.0,
                window_minutes: Some(120),
                resets_at: Some(200),
            }),
            secondary: None,
            credits: Some(CreditsSnapshot {
                has_credits: true,
                unlimited: false,
                balance: Some("50".to_string()),
            }),
            individual_limit: Some(SpendControlLimitSnapshot {
                limit: "25000".to_string(),
                used: "8000".to_string(),
                remaining_percent: 68,
                resets_at: 300,
            }),
            spend_control_reached: Some(true),
            plan_type: Some(codex_protocol::account::PlanType::Plus),
            rate_limit_reached_type: None,
        })
    );

    state.set_rate_limits(RateLimitSnapshot {
        limit_id: Some("codex_other".to_string()),
        limit_name: None,
        normal_model_slug: None,
        primary: None,
        secondary: None,
        credits: None,
        individual_limit: None,
        spend_control_reached: Some(false),
        plan_type: None,
        rate_limit_reached_type: None,
    });

    assert_eq!(
        state
            .latest_rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.spend_control_reached),
        Some(false)
    );
}
