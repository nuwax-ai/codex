use super::session::Session;
use super::turn_context::TurnContext;
use crate::config::Config;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::openai_models::ModelInfo;

#[derive(Debug)]
pub(crate) struct ContextWindowTokenStatus {
    // Full active context usage, independent of the configured auto-compact scope.
    pub(crate) active_context_tokens: i64,
    // Usage counted against `model_auto_compact_token_limit` for the current scope.
    pub(crate) auto_compact_scope_tokens: i64,
    pub(crate) auto_compact_scope_limit: Option<i64>,
    pub(crate) full_context_window_limit: Option<i64>,
    pub(crate) base_window_tokens_remaining: Option<i64>,
    pub(crate) auto_compact_window_prefill_tokens: Option<i64>,
    pub(crate) full_context_window_limit_reached: bool,
    pub(crate) token_limit_reached: bool,
    pub(crate) turn_end_compaction_threshold_reached: bool,
    /// Fork (nuwax-codex): the configured mid-turn line is crossed, so a
    /// running tool loop should compact before its next sampling request.
    pub(crate) mid_turn_compaction_threshold_reached: bool,
}

impl ContextWindowTokenStatus {
    /// True when this turn's pending inputs — items that will be recorded into
    /// history only after the pre-turn compaction check has decided — once
    /// charged would reach the model's full context window.
    ///
    /// The per-turn check otherwise sees only the previous turn's reported
    /// usage, so a single large paste can step from below every threshold over
    /// the gateway's real limit in one turn (fork probe 2026-11-02: 815,786
    /// reported usage plus ~163k pending input against a 900k full-window
    /// limit on a 1M-window gateway, rejected upstream with `1261 prompt is
    /// too long`). Reported usage itself stays untouched; this is only the
    /// compaction trigger's view of what is about to land.
    ///
    /// Only the full window is charged, never the operator's auto-compact
    /// threshold: stepping over that softer line just delays compaction to
    /// the next turn-boundary check, while stepping over the model's real
    /// window is what gets the request rejected. The derived auto-compact
    /// scope equals the full window whenever no explicit threshold narrows
    /// it, so the common configuration keeps both guards.
    pub(crate) fn pending_input_reaches_limit(&self, pending_input_tokens: i64) -> bool {
        if pending_input_tokens <= 0 {
            return false;
        }
        let full_projected = self.active_context_tokens.checked_add(pending_input_tokens);
        self.full_context_window_limit
            .zip(full_projected)
            .is_some_and(|(limit, projected)| projected >= limit)
    }
}

fn tokens_remaining(limit: Option<i64>, used: i64) -> Option<i64> {
    limit.map(|limit| limit.saturating_sub(used).max(0))
}

/// True when `active` reaches `percent` of `limit` (0 disables the check).
fn percent_threshold_reached(active: i64, limit: i64, percent: u8) -> bool {
    percent > 0 && i128::from(active) * 100 >= i128::from(limit) * i128::from(percent)
}

pub(crate) async fn context_window_token_status(
    sess: &Session,
    turn_context: &TurnContext,
) -> ContextWindowTokenStatus {
    context_window_token_status_with_config(
        sess,
        turn_context.config.as_ref(),
        turn_context.model_info().as_ref(),
    )
    .await
}

pub(crate) async fn context_window_token_status_for_model(
    sess: &Session,
    config: &Config,
    turn_context: &TurnContext,
    model_info: &ModelInfo,
) -> ContextWindowTokenStatus {
    let mut config = config.clone();
    config.token_budget = super::token_budget::resolve_token_budget(
        turn_context.configured_token_budget.as_ref(),
        turn_context.use_model_token_budget_defaults,
        model_info,
    );
    context_window_token_status_with_config(sess, &config, model_info).await
}

async fn context_window_token_status_with_config(
    sess: &Session,
    config: &Config,
    model_info: &ModelInfo,
) -> ContextWindowTokenStatus {
    let active_context_tokens = sess.get_total_token_usage().await;

    // Count either the full active context or only the tokens added after the initial prefix.
    let (auto_compact_scope_tokens, auto_compact_scope_limit, auto_compact_window_prefill_tokens) =
        match config.model_auto_compact_token_limit_scope {
            AutoCompactTokenLimitScope::Total => (
                active_context_tokens,
                model_info.auto_compact_token_limit(),
                None,
            ),
            AutoCompactTokenLimitScope::BodyAfterPrefix => {
                let window = sess.auto_compact_window_snapshot().await;
                let baseline = window.prefill_input_tokens.unwrap_or(active_context_tokens);

                let scope_limit = config
                    .model_auto_compact_token_limit
                    .or_else(|| model_info.auto_compact_token_limit());
                (
                    active_context_tokens.saturating_sub(baseline),
                    scope_limit,
                    window.prefill_input_tokens,
                )
            }
        };

    // The model's full context window is a hard cap, independent of the auto-compaction scope.
    let full_context_window_limit = model_info.resolved_context_window().map(|context_window| {
        context_window.saturating_mul(model_info.effective_context_window_percent) / 100
    });

    // Report remaining tokens against the base (unbuffered) window, capped by the full context.
    let base_window_tokens_remaining = [
        tokens_remaining(auto_compact_scope_limit, auto_compact_scope_tokens),
        tokens_remaining(full_context_window_limit, active_context_tokens),
    ]
    .into_iter()
    .flatten()
    .min();

    // Only reserve the fallback buffer when there is a fallback prompt to use it.
    let auto_compact_fallback_buffer_tokens = config
        .token_budget
        .as_ref()
        .map_or(0, crate::config::TokenBudgetConfig::fallback_buffer_tokens);
    let buffered_auto_compact_limit = auto_compact_scope_limit
        .map(|limit| limit.saturating_add(auto_compact_fallback_buffer_tokens));

    // Force compaction once the buffered window or the model's full context window is reached.
    let full_context_window_limit_reached =
        full_context_window_limit.is_some_and(|limit| active_context_tokens >= limit);
    let token_limit_reached = buffered_auto_compact_limit
        .is_some_and(|limit| auto_compact_scope_tokens >= limit)
        || full_context_window_limit_reached;
    let post_turn_percent = config.model_post_turn_compact_threshold_percent;
    let turn_end_compaction_threshold_reached = post_turn_percent > 0
        && (token_limit_reached
            || full_context_window_limit.is_some_and(|limit| {
                percent_threshold_reached(active_context_tokens, limit, post_turn_percent)
            }));
    // Fork (nuwax-codex): the same percentage check, but for a turn that is
    // still running. Hosted async compaction is unavailable on third-party
    // gateways, so without this the only mid-turn compaction was the one-shot
    // overflow recovery (after the gateway had already rejected a request).
    let mid_turn_percent = config.model_mid_turn_compact_threshold_percent;
    let mid_turn_compaction_threshold_reached = mid_turn_percent > 0
        && (token_limit_reached
            || full_context_window_limit.is_some_and(|limit| {
                percent_threshold_reached(active_context_tokens, limit, mid_turn_percent)
            }));

    ContextWindowTokenStatus {
        active_context_tokens,
        auto_compact_scope_tokens,
        auto_compact_scope_limit,
        full_context_window_limit,
        base_window_tokens_remaining,
        auto_compact_window_prefill_tokens,
        full_context_window_limit_reached,
        token_limit_reached,
        turn_end_compaction_threshold_reached,
        mid_turn_compaction_threshold_reached,
    }
}

#[cfg(test)]
mod pending_input_tests {
    use super::*;

    fn status(
        active_context_tokens: i64,
        auto_compact_scope_limit: Option<i64>,
        full_context_window_limit: Option<i64>,
    ) -> ContextWindowTokenStatus {
        ContextWindowTokenStatus {
            active_context_tokens,
            auto_compact_scope_tokens: active_context_tokens,
            auto_compact_scope_limit,
            full_context_window_limit,
            base_window_tokens_remaining: None,
            auto_compact_window_prefill_tokens: None,
            full_context_window_limit_reached: false,
            token_limit_reached: false,
            turn_end_compaction_threshold_reached: false,
            mid_turn_compaction_threshold_reached: false,
        }
    }

    #[test]
    fn percent_threshold_boundaries() {
        assert!(!percent_threshold_reached(89_999, 100_000, 90));
        assert!(percent_threshold_reached(90_000, 100_000, 90));
        assert!(percent_threshold_reached(100_000, 100_000, 90));
        // Zero disables the checkpoint even at/over the limit.
        assert!(!percent_threshold_reached(100_000, 100_000, 0));
    }

    #[test]
    fn pending_input_crossing_the_full_window_is_reached() {
        // The fork probe's shape: 815,786 reported usage, ~163k pending input,
        // a 900k full-window limit (90% of the configured 1M) — below on its
        // own, over once charged.
        let status = status(815_786, Some(900_000), Some(900_000));
        assert!(status.pending_input_reaches_limit(163_000));
    }

    #[test]
    fn pending_input_below_the_window_is_not_reached() {
        let status = status(880_000, Some(900_000), Some(900_000));
        assert!(!status.pending_input_reaches_limit(15_000));
    }

    #[test]
    fn no_pending_input_decides_nothing() {
        let status = status(895_000, Some(900_000), Some(900_000));
        assert!(!status.pending_input_reaches_limit(0));
        assert!(!status.pending_input_reaches_limit(-1));
    }

    #[test]
    fn a_soft_auto_compact_threshold_alone_does_not_charge_pending() {
        // Stepping over the operator's comfort threshold only delays
        // compaction to the next turn boundary; the full window is what the
        // pending charge guards. Here usage + pending crosses the 900k scope
        // limit but sits far below the model's 1.9M full window.
        let status = status(880_000, Some(900_000), Some(1_900_000));
        assert!(!status.pending_input_reaches_limit(50_000));
    }

    #[test]
    fn the_full_window_limits_even_without_a_compact_limit() {
        // An unknown model with no configured auto-compact limit still has the
        // hard window: charging pending input past it must reach.
        let status = status(870_000, None, Some(900_000));
        assert!(status.pending_input_reaches_limit(40_000));
        assert!(!status.pending_input_reaches_limit(15_000));
    }
}
