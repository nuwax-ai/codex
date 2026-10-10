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
        response_key: None,
        usage: usage(30),
        turn_token_usage: usage(30),
        thread_token_usage: usage(230),
    });
    let after_resume = restored.record_token_usage(
        thread_id,
        "turn-b",
        session_id,
        "root-turn".to_string(),
        /*response_key*/ None,
        "response-d",
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
            response_key: None,
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
        /*response_key*/ None,
        "response-a",
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
        /*response_key*/ None,
        "response-a",
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
        /*response_key*/ None,
        "response-b",
        &usage(10),
    );
    assert_eq!(second.turn_token_usage, usage(55));
    assert_eq!(second.thread_token_usage, usage(55));
}

#[tokio::test]
async fn same_provider_id_on_different_samplings_does_not_alias() {
    let thread_id = ThreadId::new();
    let session_id = SessionId::from(ThreadId::new());
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut state = SessionState::new(make_session_configuration_for_tests().await);

    // Two samplings whose provider echoes the SAME response id: each joins
    // the ledger by its own host key, so both count and neither replaces the
    // other.
    let first = state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        Some("sampling:turn-1:0"),
        "resp-dup",
        &usage(30),
    );
    let second = state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        Some("sampling:turn-1:1"),
        "resp-dup",
        &usage(12),
    );
    assert_eq!(first.thread_token_usage, usage(30));
    assert_eq!(
        second.thread_token_usage,
        usage(42),
        "a repeated provider id on a different sampling must not replace"
    );

    // A re-report of the FIRST sampling swaps only its own snapshot.
    let replaced = state.record_token_usage(
        thread_id,
        "turn-1",
        session_id,
        "root".to_string(),
        Some("sampling:turn-1:0"),
        "resp-dup",
        &usage(35),
    );
    assert_eq!(
        replaced.thread_token_usage,
        usage(47),
        "the host key's own snapshot is swapped, the sibling sampling stays"
    );
}

#[tokio::test]
async fn host_key_moves_between_turns_like_legacy_keys() {
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
        Some("sampling:turn-1:0"),
        "response-a",
        &usage(30),
    );
    state.record_token_usage(
        thread_id,
        "turn-2",
        session_id,
        "root".to_string(),
        Some("sampling:turn-2:0"),
        "response-b",
        &usage(10),
    );
    let replayed = state.record_token_usage(
        thread_id,
        "turn-2",
        session_id,
        "root".to_string(),
        Some("sampling:turn-1:0"),
        "response-a",
        &usage(45),
    );
    assert_eq!(
        replayed.turn_token_usage,
        usage(55),
        "moving a host-keyed snapshot into a new turn preserves the other sampling"
    );
    assert_eq!(replayed.thread_token_usage, usage(55));
}

#[test]
fn ledger_key_separates_host_keys_from_provider_ids() {
    let base = TokenUsageRecord {
        thread_id: ThreadId::new(),
        turn_id: "turn-1".to_string(),
        session_id: SessionId::from(ThreadId::new()),
        root_turn_id: "turn-1".to_string(),
        response_id: "sampling:turn-1:0".to_string(),
        response_key: None,
        usage: TokenUsage::default(),
        turn_token_usage: TokenUsage::default(),
        thread_token_usage: TokenUsage::default(),
    };
    // A provider id that literally collides with a minted key shape stays
    // in the provider-id namespace: the two records never share an entry.
    let provider_spoof = ResponseLedgerKey::of(&base).expect("joins by provider id");
    let host_keyed = ResponseLedgerKey::of(&TokenUsageRecord {
        response_key: Some("sampling:turn-1:0".to_string()),
        response_id: "anything".to_string(),
        ..base.clone()
    })
    .expect("joins by host key");
    assert_ne!(provider_spoof, host_keyed);
    assert_eq!(
        host_keyed,
        ResponseLedgerKey::Sampling("sampling:turn-1:0".to_string())
    );
    // Neither an empty id nor a key can be absent at once: such records
    // stay additive (no ledger entry).
    let keyless = ResponseLedgerKey::of(&TokenUsageRecord {
        response_key: None,
        response_id: String::new(),
        ..base
    });
    assert_eq!(keyless, None);
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
            /*response_key*/ None,
            "",
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
        /*response_key*/ None,
        "response-a",
        &usage(30),
    );
    state.record_token_usage(
        thread_id,
        "turn-2",
        session_id,
        "root".to_string(),
        /*response_key*/ None,
        "response-b",
        &usage(10),
    );
    // A new turn starts a fresh turn subtotal while the thread keeps its
    // total; re-report of turn-1's response must not inflate turn-2.
    let replayed = state.record_token_usage(
        thread_id,
        "turn-2",
        session_id,
        "root".to_string(),
        /*response_key*/ None,
        "response-a",
        &usage(45),
    );
    assert_eq!(
        replayed.turn_token_usage,
        usage(55),
        "moving a snapshot preserves other responses already in this turn"
    );
    assert_eq!(
        replayed.thread_token_usage,
        usage(55),
        "the thread total holds both latest snapshots"
    );
    let updated = state.record_token_usage(
        thread_id,
        "turn-2",
        session_id,
        "root".to_string(),
        /*response_key*/ None,
        "response-a",
        &usage(50),
    );
    assert_eq!(
        updated,
        TokenUsageRecord {
            thread_id,
            turn_id: "turn-2".to_string(),
            session_id,
            root_turn_id: "root".to_string(),
            response_id: "response-a".to_string(),
            response_key: None,
            usage: usage(50),
            turn_token_usage: usage(60),
            thread_token_usage: usage(60),
        }
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
