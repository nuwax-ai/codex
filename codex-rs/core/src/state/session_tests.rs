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

/// One ledger under test: fresh thread/session identities and a session
/// state, with a one-line record helper that spells out the report identity.
struct LedgerFixture {
    thread_id: ThreadId,
    session_id: SessionId,
    state: SessionState,
}

impl LedgerFixture {
    async fn new() -> Self {
        Self {
            thread_id: ThreadId::new(),
            session_id: SessionId::from(ThreadId::new()),
            state: SessionState::new(make_session_configuration_for_tests().await),
        }
    }

    fn record(
        &mut self,
        turn_id: &str,
        root_turn_id: &str,
        response_key: Option<&str>,
        response_id: &str,
        usage: &TokenUsage,
    ) -> TokenUsageRecord {
        self.state.record_token_usage(TokenUsageReport {
            thread_id: self.thread_id,
            turn_id,
            session_id: self.session_id,
            root_turn_id: root_turn_id.to_string(),
            response_key,
            response_id,
            usage,
        })
    }
}

#[tokio::test]
async fn record_token_usage_continues_restored_totals() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    ledger.state.latest_token_usage_record = Some(TokenUsageRecord {
        thread_id: ledger.thread_id,
        turn_id: "turn-b".to_string(),
        session_id: ledger.session_id,
        root_turn_id: "root-turn".to_string(),
        response_id: "response-c".to_string(),
        response_key: None,
        usage: usage(30),
        turn_token_usage: usage(30),
        thread_token_usage: usage(230),
    });
    let after_resume = ledger.record(
        "turn-b",
        "root-turn",
        /*response_key*/ None,
        "response-d",
        &usage(20),
    );
    assert_eq!(
        after_resume,
        TokenUsageRecord {
            thread_id: ledger.thread_id,
            turn_id: "turn-b".to_string(),
            session_id: ledger.session_id,
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
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;

    let first = ledger.record("turn-1", "root", None, "response-a", &usage(30));
    assert_eq!(first.turn_token_usage, usage(30));
    assert_eq!(first.thread_token_usage, usage(30));

    // The same response re-reports a LARGER cumulative snapshot (pause
    // continuation / duplicate delivery): the totals swap the old snapshot
    // for the new one instead of adding on top.
    let replaced = ledger.record("turn-1", "root", None, "response-a", &usage(45));
    assert_eq!(replaced.turn_token_usage, usage(45));
    assert_eq!(replaced.thread_token_usage, usage(45));

    // A second response in the same turn still adds.
    let second = ledger.record("turn-1", "root", None, "response-b", &usage(10));
    assert_eq!(second.turn_token_usage, usage(55));
    assert_eq!(second.thread_token_usage, usage(55));
}

#[tokio::test]
async fn same_provider_id_on_different_samplings_does_not_alias() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;

    // Two samplings whose provider echoes the SAME response id: each joins
    // the ledger by its own host key, so both count and neither replaces the
    // other.
    let first = ledger.record(
        "turn-1",
        "root",
        Some("sampling:turn-1:0"),
        "resp-dup",
        &usage(30),
    );
    let second = ledger.record(
        "turn-1",
        "root",
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
    let replaced = ledger.record(
        "turn-1",
        "root",
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
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    ledger.record(
        "turn-1",
        "root",
        Some("sampling:turn-1:0"),
        "response-a",
        &usage(30),
    );
    ledger.record(
        "turn-2",
        "root",
        Some("sampling:turn-2:0"),
        "response-b",
        &usage(10),
    );
    let replayed = ledger.record(
        "turn-2",
        "root",
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

#[test]
fn replayed_over_budget_identities_never_join_the_ledger() {
    // OLD rollouts can carry identities written before the live budget
    // existed. On resume they must not become join keys — nothing minted by
    // the live path could match them and truncating would alias distinct
    // ids — while budget-boundary identities still join.
    let base = TokenUsageRecord {
        thread_id: ThreadId::new(),
        turn_id: "turn-1".to_string(),
        session_id: SessionId::from(ThreadId::new()),
        root_turn_id: "turn-1".to_string(),
        response_id: String::new(),
        response_key: None,
        usage: TokenUsage::default(),
        turn_token_usage: TokenUsage::default(),
        thread_token_usage: TokenUsage::default(),
    };
    let boundary = "k".repeat(MAX_RESPONSE_IDENTITY_BYTES);
    assert_eq!(
        ResponseLedgerKey::of(&TokenUsageRecord {
            response_key: Some(boundary.clone()),
            ..base.clone()
        }),
        Some(ResponseLedgerKey::Sampling(boundary.clone()))
    );
    assert_eq!(
        ResponseLedgerKey::of(&TokenUsageRecord {
            response_id: boundary,
            ..base.clone()
        }),
        Some(ResponseLedgerKey::ProviderResponseId(
            "k".repeat(MAX_RESPONSE_IDENTITY_BYTES)
        ))
    );
    let giant_host_key = "h".repeat(MAX_RESPONSE_IDENTITY_BYTES + 1);
    assert_eq!(
        ResponseLedgerKey::of(&TokenUsageRecord {
            response_key: Some(giant_host_key),
            response_id: "still-no-join".to_string(),
            ..base.clone()
        }),
        None,
        "an over-limit host key must not fall back to its provider id either: \
         the live path could never have minted that key"
    );
    assert_eq!(
        ResponseLedgerKey::of(&TokenUsageRecord {
            response_id: "p".repeat(MAX_RESPONSE_IDENTITY_BYTES + 7),
            ..base
        }),
        None
    );
}

#[test]
fn restored_usage_records_normalize_identities_without_changing_counters() {
    let usage = TokenUsage {
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        ..TokenUsage::default()
    };
    let base = TokenUsageRecord {
        thread_id: ThreadId::new(),
        turn_id: "turn-1".to_string(),
        session_id: SessionId::from(ThreadId::new()),
        root_turn_id: "turn-1".to_string(),
        response_id: String::new(),
        response_key: None,
        usage: usage.clone(),
        turn_token_usage: usage.clone(),
        thread_token_usage: usage,
    };
    let giant = "p".repeat(MAX_RESPONSE_IDENTITY_BYTES + 1);
    let boundary = "я".repeat(MAX_RESPONSE_IDENTITY_BYTES / 2);
    for (response_key, response_id, expected_key, expected_id) in [
        (
            Some("sampling:0".to_string()),
            giant.clone(),
            Some("sampling:0".to_string()),
            String::new(),
        ),
        (None, giant.clone(), None, String::new()),
        (Some(giant), "provider".to_string(), None, String::new()),
        (
            Some(boundary.clone()),
            boundary.clone(),
            Some(boundary.clone()),
            boundary,
        ),
    ] {
        let original = TokenUsageRecord {
            response_key,
            response_id,
            ..base.clone()
        };
        let restored = crate::state::clone_usage_record_for_resume(&original);
        assert_eq!(
            restored,
            TokenUsageRecord {
                response_key: expected_key,
                response_id: expected_id,
                ..base.clone()
            }
        );
        assert_eq!(
            ResponseLedgerKey::of(&restored),
            ResponseLedgerKey::of(&original)
        );
    }
}

#[tokio::test]
async fn restored_bounded_host_key_keeps_snapshot_replacement() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    let mut original = ledger.record("turn-1", "root", Some("sampling:0"), "provider", &usage(30));
    original.response_id = "p".repeat(MAX_RESPONSE_IDENTITY_BYTES + 1);
    original.thread_token_usage = usage(230);
    let restored = crate::state::clone_usage_record_for_resume(&original);
    ledger.state.latest_token_usage_record = Some(restored.clone());
    ledger.state.response_token_usage.insert(
        ResponseLedgerKey::of(&restored).expect("bounded host key still joins"),
        restored,
    );
    let replaced = ledger.record("turn-1", "root", Some("sampling:0"), "provider", &usage(35));
    assert_eq!(
        replaced,
        TokenUsageRecord {
            response_id: "provider".to_string(),
            usage: usage(35),
            turn_token_usage: usage(35),
            thread_token_usage: usage(235),
            ..original
        }
    );
}

#[tokio::test]
async fn keyless_reports_stay_additive() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    // Keyless snapshots cannot replace anything: each report adds.
    for (report, expected_total) in [(10, 10), (25, 35), (25, 60)] {
        let record = ledger.record("turn-1", "root", None, "", &usage(report));
        assert_eq!(record.turn_token_usage, usage(expected_total));
        assert_eq!(record.thread_token_usage, usage(expected_total));
    }
}

#[tokio::test]
async fn over_budget_provider_response_ids_stay_keyless_additive() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    // Two over-budget ids sharing an entire prefix: neither is stored nor
    // keyed — never truncated into a prefix join — so both reports stay
    // additive and the records carry an empty id.
    let giant_a = "a".repeat(MAX_RESPONSE_IDENTITY_BYTES + 1);
    let mut giant_b = giant_a.clone();
    giant_b.push('b');
    let first = ledger.record("turn-1", "root", None, &giant_a, &usage(30));
    let second = ledger.record("turn-1", "root", None, &giant_b, &usage(12));
    assert_eq!(first.response_id, "");
    assert_eq!(second.response_id, "");
    assert_eq!(second.thread_token_usage, usage(42));
    assert!(
        ledger.state.response_token_usage.is_empty(),
        "over-budget ids must not join the ledger"
    );
    // In-budget provider ids keep the legacy replace semantics.
    let replaced = ledger.record("turn-1", "root", None, "resp-normal", &usage(50));
    let replay = ledger.record("turn-1", "root", None, "resp-normal", &usage(60));
    assert_eq!(replaced.response_id, "resp-normal");
    assert_eq!(replay.thread_token_usage, usage(102));
}

#[tokio::test]
async fn host_keyed_reports_join_regardless_of_provider_id_length() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    // A giant provider id cannot unseat a host-keyed sampling's join: the
    // id degrades to empty in storage while the host key keeps replacing.
    let giant = "g".repeat(MAX_RESPONSE_IDENTITY_BYTES + 7);
    ledger.record(
        "turn-1",
        "root",
        Some("sampling:turn-1:0"),
        &giant,
        &usage(30),
    );
    let replay = ledger.record(
        "turn-1",
        "root",
        Some("sampling:turn-1:0"),
        &giant,
        &usage(45),
    );
    assert_eq!(replay.response_id, "");
    assert_eq!(replay.response_key.as_deref(), Some("sampling:turn-1:0"));
    assert_eq!(replay.thread_token_usage, usage(45));
}

#[tokio::test]
async fn ledger_survives_into_new_turns_without_double_counting() {
    let usage = |total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    };
    let mut ledger = LedgerFixture::new().await;
    ledger.record("turn-1", "root", None, "response-a", &usage(30));
    ledger.record("turn-2", "root", None, "response-b", &usage(10));
    // A new turn starts a fresh turn subtotal while the thread keeps its
    // total; re-report of turn-1's response must not inflate turn-2.
    let replayed = ledger.record("turn-2", "root", None, "response-a", &usage(45));
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
    let updated = ledger.record("turn-2", "root", None, "response-a", &usage(50));
    assert_eq!(
        updated,
        TokenUsageRecord {
            thread_id: ledger.thread_id,
            turn_id: "turn-2".to_string(),
            session_id: ledger.session_id,
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
