use crate::context::UserGoalUpdate;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use crate::Prompt;
use crate::client::ModelClientSession;
use crate::client_common::ResponseEvent;
use crate::context::BaseInstructionsFragment;
use crate::context::CompactionSummary;
use crate::context::ContextualUserFragment;
use crate::context::world_state::WorldState;
use crate::context::world_state::WorldStateSnapshot;
use crate::context::world_state::split_prefix_updates;
use crate::context_manager::updates::merge_world_state_updates;
use crate::hook_runtime::PostCompactHookOutcome;
use crate::hook_runtime::PreCompactHookOutcome;
use crate::hook_runtime::run_post_compact_hooks;
use crate::hook_runtime::run_pre_compact_hooks;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CompactionTurnMetadata;
use crate::session::RequestEffortUsage;
use crate::session::TokenUsageContext;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn::get_last_assistant_message_from_turn;
use crate::session::turn_context::TurnContext;
use crate::state::AutoCompactWindowIds;
use crate::util::backoff;
use codex_analytics::CodexCompactionEvent;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionStatus;
use codex_analytics::CompactionStrategy;
use codex_analytics::CompactionTrigger;
use codex_analytics::now_unix_seconds;
use codex_context_fragments::AnnotatedContent;
use codex_context_fragments::set_annotated_content;
use codex_context_fragments::truncate_annotated_text;
use codex_history::CodexHarnessMetadata;
use codex_history::ResponseItemEnvelope;
use codex_protocol::ResponseItemId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use codex_protocol::user_input::UserInput;
use codex_rollout_trace::InferenceTraceContext;
use codex_utils_output_truncation::approx_token_count;
use futures::prelude::*;
use tracing::error;

pub use codex_prompts::SUMMARIZATION_PROMPT;
pub use codex_prompts::SUMMARY_PREFIX;
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Output tokens reserved for one migration-compaction summary. Charged
/// against the target window when computing the history budget in
/// `run_migration_compact_task` AND enforced as the summary request's actual
/// output cap in `summarize_migration_region` — one value, both sides.
pub(crate) const RESERVED_OUTPUT_TOKENS: i64 = 2_048;

/// Maximum number of "remove the oldest item" retries inside one compaction
/// before the caller's degrade path (omit oldest history + persist a
/// `SurfaceReplacement`) takes over. Each round is a request the gateway has
/// already rejected once, so the loop must stay bounded; two rounds also keep
/// the retry count deterministic for the mock-server tests.
const MAX_OVERFLOW_TRIM_ROUNDS: u8 = 2;

/// Bound the "halve the largest output" loop. The loop also stops on its own
/// when no application shrinks the payload anymore, so the cap only guards
/// pathological churn; many small outputs each need one round.
const MAX_PRUNE_ESCALATION_ROUNDS: u8 = 32;

/// Thread-local policy that preserves inherited context without changing model or skill budgets.
/// Insert into `StartThreadOptions::thread_extension_init` to disable automatic compaction.
pub struct AutoCompactionDisabled;

/// Metadata for a new compaction checkpoint, kept separate from its replacement history.
///
/// `Session::replace_compacted_history` assigns missing item IDs before constructing the persisted
/// `CompactedItem`, ensuring the live and persisted histories remain identical.
pub(crate) struct CompactedHistoryMetadata {
    pub(crate) input_goal_ids: HashSet<ResponseItemId>,
    pub(crate) message: String,
    pub(crate) window_number: u64,
    pub(crate) window_ids: AutoCompactWindowIds,
    pub(crate) compaction_response_id: Option<String>,
    pub(crate) compaction_model_hash: Option<String>,
    pub(crate) reviewer_compaction_hash: Option<String>,
}

/// Renders the window prefix and ordinary context with their comparison baseline.
pub(crate) async fn build_compaction_replacement_history(
    sess: &Session,
    step_context: &StepContext,
    world_state: &WorldState,
    compacted_history: Vec<ResponseItemEnvelope>,
) -> (Vec<ResponseItemEnvelope>, WorldStateSnapshot) {
    let (updates, snapshot) = sess
        .build_initial_context_with_world_state(step_context, world_state)
        .await;
    let (prefix, context) = split_prefix_updates(updates);
    let context = merge_world_state_updates(context);
    (
        assemble_compaction_history(compacted_history, prefix, context),
        snapshot,
    )
}

fn assemble_compaction_history(
    compacted_history: Vec<ResponseItemEnvelope>,
    prefix: Vec<ResponseItem>,
    context: Vec<ResponseItem>,
) -> Vec<ResponseItemEnvelope> {
    let history = insert_initial_context_before_last_real_user_or_summary(
        compacted_history,
        context.into_iter().map(ResponseItemEnvelope::new).collect(),
    );
    prefix
        .into_iter()
        .map(ResponseItemEnvelope::new)
        .chain(history)
        .collect()
}

pub(crate) async fn run_inline_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    replacement_step_context: Arc<StepContext>,
    world_state: Arc<WorldState>,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let prompt = turn_context
        .config
        .compact_prompt
        .as_deref()
        .unwrap_or(SUMMARIZATION_PROMPT)
        .to_string();
    let input = vec![UserInput::Text {
        text: prompt,
        // Compaction prompt is synthesized; no UI element ranges to preserve.
        text_elements: Vec::new(),
    }];

    run_compact_task_inner(
        sess,
        turn_context,
        replacement_step_context,
        input,
        world_state,
        CompactionTurnMetadata::new(
            CompactionTrigger::Auto,
            reason,
            CompactionImplementation::Responses,
            phase,
        ),
    )
    .await?;
    Ok(())
}

pub(crate) async fn run_compact_task(
    sess: Arc<Session>,
    step_context: Arc<StepContext>,
    world_state: Arc<WorldState>,
    input: Vec<UserInput>,
) -> CodexResult<()> {
    run_compact_task_inner(
        sess.clone(),
        Arc::clone(&step_context.turn),
        step_context,
        input,
        world_state,
        CompactionTurnMetadata::new(
            CompactionTrigger::Manual,
            CompactionReason::UserRequested,
            CompactionImplementation::Responses,
            CompactionPhase::StandaloneTurn,
        ),
    )
    .await?;
    Ok(())
}

async fn run_compact_task_inner(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    replacement_step_context: Arc<StepContext>,
    input: Vec<UserInput>,
    world_state: Arc<WorldState>,
    compaction_metadata: CompactionTurnMetadata,
) -> CodexResult<()> {
    let trigger = compaction_metadata.trigger();
    let reason = compaction_metadata.reason();
    let phase = compaction_metadata.phase();
    let attempt = CompactionAnalyticsAttempt::begin(
        sess.as_ref(),
        turn_context.as_ref(),
        trigger,
        reason,
        CompactionImplementation::Responses,
        phase,
    )
    .await;
    let pre_compact_outcome = run_pre_compact_hooks(&sess, &turn_context, trigger).await;
    match pre_compact_outcome {
        PreCompactHookOutcome::Continue => {}
        PreCompactHookOutcome::Stopped => {
            let error = CodexErr::TurnAborted;
            attempt
                .track(
                    sess.as_ref(),
                    CompactionStatus::Interrupted,
                    Some(&error),
                    CompactionAnalyticsDetails::default(),
                )
                .await;
            return Err(error);
        }
    }
    let result = run_compact_task_inner_impl(
        Arc::clone(&sess),
        Arc::clone(&turn_context),
        replacement_step_context,
        input,
        world_state,
        compaction_metadata,
    )
    .await;
    let status = compaction_status_from_result(&result);
    let codex_error = result.as_ref().err();
    if result.is_ok() {
        let post_compact_outcome = run_post_compact_hooks(&sess, &turn_context, trigger).await;
        if let PostCompactHookOutcome::Stopped = post_compact_outcome {
            attempt
                .track(
                    sess.as_ref(),
                    status,
                    codex_error,
                    CompactionAnalyticsDetails::default(),
                )
                .await;
            return Err(CodexErr::TurnAborted);
        }
    }
    attempt
        .track(
            sess.as_ref(),
            status,
            codex_error,
            CompactionAnalyticsDetails::default(),
        )
        .await;
    if let Err(err) = &result
        && !matches!(phase, CompactionPhase::PostTurn)
        && !matches!(
            err.details(),
            CodexErrorDetails::Interrupted | CodexErrorDetails::TurnAborted
        )
    {
        sess.track_turn_codex_error(turn_context.as_ref(), err);
        // Pre-turn failures are reported after preserving the incoming prompt.
        if !matches!(phase, CompactionPhase::PreTurn) {
            let event = EventMsg::Error(err.to_error_event(/*message_prefix*/ None));
            sess.send_event(&turn_context, event).await;
        }
    }
    result.map(|_| ())
}

async fn run_compact_task_inner_impl(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    replacement_step_context: Arc<StepContext>,
    input: Vec<UserInput>,
    world_state: Arc<WorldState>,
    compaction_metadata: CompactionTurnMetadata,
) -> CodexResult<String> {
    let compaction_item = TurnItem::ContextCompaction(ContextCompactionItem::new());
    sess.emit_turn_item_started(&turn_context, &compaction_item)
        .await;
    let initial_input_for_turn: ResponseInputItem = ResponseInputItem::from(input);

    let mut history = sess.clone_history().await;
    let input_goal_ids = UserGoalUpdate::message_ids(history.raw_items());
    history.record_items(
        &[initial_input_for_turn.into()],
        turn_context.model_info().truncation_policy.into(),
    );
    let mut fallback_instructions = sess.get_prompt_base_instructions().await;
    if let Some(text) = history
        .raw_items()
        .find_map(BaseInstructionsFragment::text_from_item)
    {
        fallback_instructions.text = text.to_owned();
    } else if history.has_tool_declarations() {
        // An incremental window has no recorded base item when its instructions are empty.
        fallback_instructions.text.clear();
    }

    let max_retries = turn_context.provider.info().stream_max_retries();
    let mut retries = 0;
    // Fork (2026-11-09): overflow remediation is bounded. Each retry is a
    // request the gateway has already rejected once, so an unbounded
    // one-item-at-a-time loop can spend dozens of them before giving up. The
    // per-failure drop stays "oldest item only" (tool call/output pairs are
    // preserved; `build_compacted_history` re-injects the base instructions),
    // but after `MAX_OVERFLOW_TRIM_ROUNDS` the caller's degrade path takes
    // over instead of retrying forever.
    let mut overflow_pruned = false;
    let mut overflow_trim_rounds = 0u8;
    // Reuse one client session so turn-scoped state (sticky routing and websocket incremental
    // request tracking) survives retries within this compact turn.
    let mut client_session = sess.services.model_client.new_session();
    let compaction_response = loop {
        // Clone is required because of the loop
        let annotated_input = history
            .clone()
            .for_prompt_annotated(
                &turn_context.model_info().input_modalities,
                sess.services.image_store.as_ref(),
            )
            .await;
        let input_provenance = crate::model_output_projection::sources_for_input(&annotated_input);
        let mut turn_input = annotated_input
            .into_iter()
            .map(codex_history::ResponseItemEnvelope::into_item)
            .collect::<Vec<_>>();
        sess.services
            .executed_tool_calls
            .attach_to_compaction_prompt(&mut turn_input);
        let turn_input_len = turn_input.len();
        let base_instructions = if turn_input
            .iter()
            .any(|item| BaseInstructionsFragment::text_from_item(item).is_some())
        {
            BaseInstructions {
                text: String::new(),
                provenance: None,
            }
        } else {
            fallback_instructions.clone()
        };
        let prompt = Prompt {
            input: turn_input,
            input_provenance,
            base_instructions,
            cyber_access_program: turn_context.cyber_access_program,
            ..Default::default()
        };
        let responses_metadata = sess
            .compaction_responses_metadata(turn_context.as_ref(), compaction_metadata)
            .await;
        // Each wire attempt is its own sampling: mint a fresh host key so
        // the ledger joins this compaction attempt by host identity instead
        // of the provider response id, which a provider may reuse across
        // distinct compactions.
        let response_key = sess.next_sampling_response_key(&turn_context.sub_id);
        let attempt_result = drain_to_completed(
            &sess,
            turn_context.as_ref(),
            &mut client_session,
            &responses_metadata,
            &prompt,
            compaction_metadata.phase(),
            &response_key,
        )
        .await;

        match attempt_result {
            Ok(response) => {
                break response;
            }
            Err(err)
                if matches!(
                    err.details(),
                    CodexErrorDetails::Interrupted | CodexErrorDetails::TurnAborted
                ) =>
            {
                return Err(err);
            }
            Err(e)
                if matches!(
                    e.details(),
                    CodexErrorDetails::SessionBudgetExceeded
                        | CodexErrorDetails::CapExhausted { .. }
                        | CodexErrorDetails::ResponseIncomplete(_)
                        | CodexErrorDetails::Fatal(_)
                ) =>
            {
                // A cap terminal is not a transport failure: retrying the
                // same exhausted generation budget cannot recover.
                return Err(e);
            }
            Err(e) if matches!(e.details(), CodexErrorDetails::ContextWindowExceeded) => {
                // Fork (2026-11-09): shrink deterministically before losing
                // history. Oversized text tool results (greps, HTML dumps,
                // log tails) dominate long agent histories; cutting them to a
                // head/tail window costs no model call and keeps every message
                // and call/output pair intact for the summarizer.
                if !overflow_pruned {
                    overflow_pruned = true;
                    let pruned = crate::session::compaction_prune::prune_oversized_tool_results(
                        history.annotated_items_mut(),
                        /*keep_head_chars*/ 6_000,
                        /*keep_tail_chars*/ 2_000,
                        /*min_chars_to_prune*/ 8_000,
                    );
                    if !pruned.is_empty() {
                        error!(
                            pruned_results = pruned.pruned_results,
                            chars_removed = pruned.chars_removed,
                            "Context window exceeded while compacting; pruned oversized tool results. Error: {e}"
                        );
                        retries = 0;
                        continue;
                    }
                }
                if overflow_trim_rounds < MAX_OVERFLOW_TRIM_ROUNDS && turn_input_len > 1 {
                    overflow_trim_rounds += 1;
                    // Trim from the beginning to preserve cache (prefix-based)
                    // and keep recent messages intact.
                    error!(
                        round = overflow_trim_rounds,
                        "Context window exceeded while compacting; removing the oldest history item. Error: {e}"
                    );
                    history.remove_first_item();
                    retries = 0;
                    continue;
                }
                sess.set_total_tokens_full(turn_context.as_ref()).await;
                return Err(e);
            }
            Err(e) => {
                if retries < max_retries {
                    retries += 1;
                    let delay = backoff(retries);
                    sess.notify_stream_error(
                        turn_context.as_ref(),
                        format!("Reconnecting... {retries}/{max_retries}"),
                        e,
                    )
                    .await;
                    tokio::time::sleep(delay).await;
                    continue;
                } else {
                    return Err(e);
                }
            }
        }
    };

    let history_snapshot = sess.clone_history().await;
    let history_items = history_snapshot.annotated_items();
    let summary_suffix = if matches!(compaction_metadata.phase(), CompactionPhase::PostTurn) {
        get_last_assistant_message_from_turn(compaction_response.output.iter())
            .filter(|summary| !summary.trim().is_empty())
            .ok_or_else(|| {
                CodexErr::Stream(
                    "Post-turn compaction completed without an assistant summary".to_string(),
                )
            })?
    } else {
        get_last_assistant_message_from_turn(history_snapshot.raw_items()).unwrap_or_default()
    };
    let summary_text = format!("{SUMMARY_PREFIX}\n{summary_suffix}");
    let user_messages = collect_annotated_user_messages(history_items);

    let mut new_history = build_compacted_history(Vec::new(), &user_messages, &summary_text);
    if let Some(summary_item) = new_history.last_mut() {
        // This replacement history skips `record_conversation_items`; only the appended summary
        // belongs to this compaction turn.
        summary_item.set_turn_id_if_missing(&turn_context.sub_id);
    }
    let (window_number, window_ids) = sess.advance_auto_compact_window().await;

    let (new_history, world_state_baseline) = build_compaction_replacement_history(
        sess.as_ref(),
        &replacement_step_context,
        &world_state,
        new_history,
    )
    .await;
    sess.replace_compacted_history(
        new_history,
        replacement_step_context.to_turn_context_item(),
        world_state_baseline,
        CompactedHistoryMetadata {
            input_goal_ids,
            message: summary_text,
            window_number,
            window_ids,
            compaction_response_id: Some(compaction_response.response_id),
            compaction_model_hash: turn_context.model_info().comp_hash.clone(),
            reviewer_compaction_hash: None,
        },
    )
    .await;
    sess.recompute_token_usage(&replacement_step_context.turn)
        .await;

    sess.emit_turn_item_completed(&turn_context, compaction_item)
        .await;
    Ok(summary_suffix)
}

pub(crate) struct CompactionAnalyticsAttempt {
    thread_id: String,
    turn_id: String,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    implementation: CompactionImplementation,
    phase: CompactionPhase,
    active_context_tokens_before: i64,
    started_at: u64,
    start_instant: Instant,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactionAnalyticsDetails {
    pub(crate) active_context_tokens_before: Option<i64>,
    pub(crate) retained_image_count: Option<usize>,
    pub(crate) compaction_summary_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
    pub(crate) cache_write_input_tokens: Option<i64>,
}

impl CompactionAnalyticsAttempt {
    pub(crate) async fn begin(
        sess: &Session,
        turn_context: &TurnContext,
        trigger: CompactionTrigger,
        reason: CompactionReason,
        implementation: CompactionImplementation,
        phase: CompactionPhase,
    ) -> Self {
        let active_context_tokens_before = sess.get_total_token_usage().await;
        Self {
            thread_id: sess.thread_id.to_string(),
            turn_id: turn_context.sub_id.clone(),
            trigger,
            reason,
            implementation,
            phase,
            active_context_tokens_before,
            started_at: now_unix_seconds(),
            start_instant: Instant::now(),
        }
    }

    pub(crate) async fn track(
        self,
        sess: &Session,
        status: CompactionStatus,
        codex_error: Option<&CodexErr>,
        details: CompactionAnalyticsDetails,
    ) {
        let CompactionAnalyticsDetails {
            active_context_tokens_before,
            retained_image_count,
            compaction_summary_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
        } = details;
        let active_context_tokens_before =
            active_context_tokens_before.unwrap_or(self.active_context_tokens_before);
        let active_context_tokens_after = sess.get_total_token_usage().await;
        sess.services
            .analytics_events_client
            .track_compaction(CodexCompactionEvent {
                thread_id: self.thread_id,
                turn_id: self.turn_id,
                trigger: self.trigger,
                reason: self.reason,
                implementation: self.implementation,
                phase: self.phase,
                strategy: CompactionStrategy::Memento,
                status,
                codex_error_kind: codex_error.map(Into::into),
                codex_error_http_status_code: codex_error
                    .and_then(CodexErr::http_status_code_value),
                usage_limit_window_minutes: codex_error.and_then(|error| match error.details() {
                    CodexErrorDetails::UsageLimitReached(error) => error.limit_window_minutes,
                    _ => None,
                }),
                active_context_tokens_before,
                active_context_tokens_after,
                retained_image_count,
                compaction_summary_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                started_at: self.started_at,
                completed_at: now_unix_seconds(),
                duration_ms: Some(
                    u64::try_from(self.start_instant.elapsed().as_millis()).unwrap_or(u64::MAX),
                ),
            });
    }
}

pub(crate) fn compaction_status_from_result<T>(result: &CodexResult<T>) -> CompactionStatus {
    match result {
        Ok(_) => CompactionStatus::Completed,
        Err(err)
            if matches!(
                err.details(),
                CodexErrorDetails::Interrupted | CodexErrorDetails::TurnAborted
            ) =>
        {
            CompactionStatus::Interrupted
        }
        Err(_) => CompactionStatus::Failed,
    }
}

pub fn content_items_to_text(content: &[ContentItem]) -> Option<String> {
    let mut pieces = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text, .. } => {
                if !text.is_empty() {
                    pieces.push(text.as_str());
                }
            }
            ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => {}
        }
    }
    if pieces.is_empty() {
        None
    } else {
        Some(pieces.join("\n"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompactedUserMessage<'a> {
    // Flattened text is only for budgeting and recognizing summaries. The
    // output is built from original parts, never reconstructed from this text.
    // Borrow from the history snapshot until selected output is materialized.
    message: String,
    original: &'a ResponseItem,
    harness_metadata: Option<&'a CodexHarnessMetadata>,
}

#[cfg(test)]
pub(crate) fn collect_user_messages(items: &[ResponseItem]) -> Vec<CompactedUserMessage<'_>> {
    items
        .iter()
        .filter_map(|item| compacted_user_message(item, /*harness_metadata*/ None))
        .collect()
}

pub(crate) fn collect_annotated_user_messages(
    items: &[ResponseItemEnvelope],
) -> Vec<CompactedUserMessage<'_>> {
    items
        .iter()
        .filter_map(|envelope| compacted_user_message(&envelope.item, envelope.metadata.as_ref()))
        .collect()
}

fn compacted_user_message<'a>(
    item: &'a ResponseItem,
    harness_metadata: Option<&'a CodexHarnessMetadata>,
) -> Option<CompactedUserMessage<'a>> {
    let Some(TurnItem::UserMessage(user)) = crate::event_mapping::parse_turn_item(item) else {
        return None;
    };
    let message = user.message();
    if is_summary_message(&message) {
        return None;
    }
    Some(CompactedUserMessage {
        message,
        original: item,
        harness_metadata,
    })
}

pub(crate) fn is_summary_message(message: &str) -> bool {
    message.starts_with(format!("{SUMMARY_PREFIX}\n").as_str())
}

pub(crate) fn is_real_user_or_agent_message(item: &ResponseItem) -> bool {
    if let ResponseItem::AgentMessage { content, .. } = item {
        return !matches!(
            content.first(),
            Some(AgentMessageInputContent::InputText { text })
                if text.starts_with("Message Type: FINAL_ANSWER\n")
        );
    }
    matches!(item, ResponseItem::Message { role, .. } if role == "user")
        && matches!(
            crate::event_mapping::parse_turn_item(item),
            Some(TurnItem::UserMessage(user)) if !is_summary_message(&user.message())
        )
}

/// Inserts canonical initial context into compacted replacement history at the
/// model-expected boundary.
///
/// Placement rules:
/// - Prefer immediately before the last real user, agent message, or retained standalone output.
/// - If no real user messages remain, insert before the compaction summary so
///   the summary stays last.
/// - If there are no user messages, insert before the last compaction item so
///   that item remains last (remote compaction may return only compaction items).
/// - If there are no user messages or compaction items, append the context.
pub(crate) fn insert_initial_context_before_last_real_user_or_summary(
    mut compacted_history: Vec<ResponseItemEnvelope>,
    initial_context: Vec<ResponseItemEnvelope>,
) -> Vec<ResponseItemEnvelope> {
    let mut last_user_or_summary_index = None;
    let mut last_real_user_index = None;
    for (i, item) in compacted_history.iter().enumerate().rev() {
        if crate::compact_retained_tool_outputs::is_retained_tool_output(item)
            || is_real_user_or_agent_message(&item.item)
        {
            last_real_user_index = Some(i);
            break;
        }
        // Compaction summaries are encoded as user messages, so track both:
        // the last real user message (preferred insertion point) and the last
        // user-message-like item (fallback summary insertion point).
        if matches!(
            crate::event_mapping::parse_turn_item(&item.item),
            Some(TurnItem::UserMessage(_))
        ) {
            last_user_or_summary_index.get_or_insert(i);
        }
    }
    let last_compaction_index = compacted_history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, item)| {
            matches!(
                &item.item,
                ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
            )
            .then_some(i)
        });
    let insertion_index = last_real_user_index
        .or(last_user_or_summary_index)
        .or(last_compaction_index);

    // Re-inject canonical context from the current session since we stripped it
    // from the pre-compaction history. Prefer placing it before the last real
    // user message; if there is no real user message left, place it before the
    // summary or compaction item so the compaction item remains last.
    if let Some(insertion_index) = insertion_index {
        compacted_history.splice(insertion_index..insertion_index, initial_context);
    } else {
        compacted_history.extend(initial_context);
    }

    compacted_history
}

pub(crate) fn build_compacted_history(
    initial_context: Vec<ResponseItemEnvelope>,
    user_messages: &[CompactedUserMessage<'_>],
    summary_text: &str,
) -> Vec<ResponseItemEnvelope> {
    build_compacted_history_with_limit(
        initial_context,
        user_messages,
        summary_text,
        COMPACT_USER_MESSAGE_MAX_TOKENS,
    )
}

fn build_compacted_history_with_limit(
    mut history: Vec<ResponseItemEnvelope>,
    user_messages: &[CompactedUserMessage<'_>],
    summary_text: &str,
    max_tokens: usize,
) -> Vec<ResponseItemEnvelope> {
    let mut selected_messages = Vec::new();
    if max_tokens > 0 {
        let mut remaining = max_tokens;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let tokens = approx_token_count(&message.message);
            let ResponseItem::Message {
                id,
                content,
                internal_chat_message_metadata_passthrough,
                ..
            } = message.original
            else {
                continue;
            };
            let mut harness_metadata = message.harness_metadata.cloned();
            let preserve_content = tokens <= remaining
                && content
                    .iter()
                    .all(|part| matches!(part, ContentItem::InputText { .. }));
            let mut item = ResponseItem::Message {
                status: None,
                encrypted_content: None,
                id: id.clone(),
                role: "user".to_owned(),
                content: if preserve_content {
                    content.clone()
                } else {
                    Vec::new()
                },
                phase: None,
                internal_chat_message_metadata_passthrough:
                    internal_chat_message_metadata_passthrough.clone(),
            };
            if !preserve_content {
                // Select text and its annotations by the same source index.
                // Do not clone discarded images/audio or infer their text's source.
                let metadata = internal_chat_message_metadata_passthrough.as_ref();
                let mut parts: Vec<_> = crate::event_mapping::user_input_parts(content)
                    .filter_map(|(index, part)| {
                        let ContentItem::InputText { text } = part else {
                            return None;
                        };
                        let kind = metadata
                            .and_then(|metadata| metadata.content_item_kinds.as_ref())
                            .and_then(|kinds| kinds.get(index))
                            .cloned()
                            .unwrap_or_else(|| ContentItemKind("unknown".to_string()));
                        let source = metadata
                            .and_then(|metadata| metadata.content_item_metadata.as_ref())
                            .and_then(|sources| sources.get(index))
                            .cloned()
                            .unwrap_or_default();
                        Some(AnnotatedContent::text(text, kind, source))
                    })
                    .collect();
                if parts.is_empty() {
                    // Preserve the existing empty-text fallback for media-only input.
                    parts.push(AnnotatedContent::new(
                        ContentItem::InputText {
                            text: String::new(),
                        },
                        ContentItemKind("unknown".to_string()),
                    ));
                }
                set_annotated_content(&mut item, truncate_annotated_text(parts, remaining));
            }
            if tokens > remaining
                && let Some(metadata) = &mut harness_metadata
            {
                metadata.mark_retained_sources_incomplete();
            }
            selected_messages.push(ResponseItemEnvelope {
                item,
                metadata: harness_metadata,
            });
            if tokens > remaining {
                break;
            }
            remaining = remaining.saturating_sub(tokens);
        }
        selected_messages.reverse();
    }
    history.extend(selected_messages);

    let summary_text = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text.to_string()
    };

    history.push(ResponseItemEnvelope::new(ContextualUserFragment::into(
        CompactionSummary::new(summary_text),
    )));

    history
}

struct CompactionResponse {
    response_id: String,
    output: Vec<ResponseItem>,
}

async fn drain_to_completed(
    sess: &Session,
    turn_context: &TurnContext,
    client_session: &mut ModelClientSession,
    responses_metadata: &CodexResponsesMetadata,
    prompt: &Prompt,
    phase: CompactionPhase,
    response_key: &str,
) -> CodexResult<CompactionResponse> {
    let mut includes_reasoning = turn_context.model_info().use_responses_lite;
    let mut stream = client_session
        .stream(
            prompt,
            turn_context.model_info(),
            &turn_context.session_telemetry,
            sess.reasoning_effort_for_request(
                &turn_context.initial_settings,
                RequestEffortUsage::Compaction,
            )
            .await,
            turn_context.reasoning_summary(),
            turn_context.config.service_tier.clone(),
            responses_metadata,
            // Rollout tracing currently models remote compaction only; local compaction streams
            // are left untraced until the reducer has a first-class local compaction lifecycle.
            &InferenceTraceContext::disabled(),
        )
        .await?;
    let mut output = Vec::new();
    loop {
        let maybe_event = stream.next().await;
        let Some(event) = maybe_event else {
            return Err(CodexErr::Stream(
                "stream closed before response.completed".into(),
            ));
        };
        match event {
            // Keep each attempt isolated until the response completes successfully.
            Ok(ResponseEvent::OutputItemDone(item)) => output.push(item),
            Ok(ResponseEvent::ServerReasoningIncluded(included)) => {
                includes_reasoning = included;
            }
            Ok(ResponseEvent::RateLimits(snapshot)) => {
                sess.update_rate_limits(turn_context, snapshot).await;
            }
            Ok(ResponseEvent::Completed {
                response_id,
                token_usage,
                usage_metadata,
                ..
            }) => {
                if !matches!(phase, CompactionPhase::PostTurn) {
                    for item in std::mem::take(&mut output) {
                        sess.record_annotated_conversation_items(
                            turn_context,
                            turn_context.model_info(),
                            vec![ResponseItemEnvelope {
                                item,
                                metadata: Some(CodexHarnessMetadata {
                                    compaction_output: true,
                                    ..Default::default()
                                }),
                            }],
                        )
                        .await;
                    }
                }
                sess.record_observed_response_completed(
                    turn_context,
                    Some(response_key),
                    &response_id,
                    token_usage.as_ref(),
                    usage_metadata.as_ref(),
                )
                .await;
                sess.update_token_usage_info(
                    turn_context,
                    token_usage.as_ref(),
                    includes_reasoning,
                )
                .await?;
                return Ok(CompactionResponse {
                    response_id,
                    output,
                });
            }
            Ok(_) => continue,
            Err(e) => {
                if let CodexErrorDetails::ResponseIncomplete(incomplete) = e.details() {
                    sess.record_incomplete_response_usage(
                        turn_context,
                        &turn_context.initial_settings,
                        incomplete,
                        Some(response_key),
                    )
                    .await;
                    sess.send_token_count_event(turn_context).await;
                } else if let CodexErrorDetails::CapExhausted {
                    response_id,
                    reported_usage,
                    ..
                } = e.details()
                {
                    // Fork cap terminal carries the same reported usage an
                    // incomplete response would; account it before the
                    // terminal propagates (mirrors the sampling path).
                    if let Some(usage) = sess
                        .record_cap_exhausted_usage(
                            turn_context,
                            response_key,
                            response_id.as_deref(),
                            reported_usage.as_ref(),
                        )
                        .await
                        && let Err(accounting_error) = sess
                            .record_token_usage_info(
                                turn_context,
                                &turn_context.initial_settings,
                                Some(&usage),
                                TokenUsageContext::SpendingOnly,
                            )
                            .await
                    {
                        tracing::warn!(
                            error = %accounting_error,
                            "failed to account a capped compaction's usage"
                        );
                    }
                    sess.send_token_count_event(turn_context).await;
                }
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;

/// The outcome of one migration-compaction pass (downshift plan 2026-11-08).
///
/// The caller distinguishes normal convergence (proceed to inference), the
/// degradable states (history still over budget after `max_ranges` bounded
/// spans, or summarization unavailable — trim older context on the target
/// model instead), and user cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationOutcome {
    /// Nothing to migrate: window unknown, or the history already fits.
    NoMigrationNeeded,
    /// The history fits after zero or more bounded summary ranges.
    Converged,
    /// Summaries made progress but the history still exceeds the budget
    /// after this pass's `max_ranges` ranges: degrade by trimming.
    StillOverBudget,
    /// No legal cut, no identified boundary, a failed or empty summary, or a
    /// pass that stopped shrinking the history: degradation is the only way
    /// to continue on the target model.
    CannotSummarize,
    /// The user cancelled between ranges; already-persisted records stay.
    Cancelled,
}

/// Migration compaction: fit the conversation into the current model's window
/// using only the current model.
///
/// A conversation that grew on a large-window model cannot be handed to a small
/// one by asking the old model to summarize it (that model may be gone). Instead
/// each step replaces one bounded, pairing-safe range with a summary the *new*
/// model writes, then remeasures. The bound is what makes this work: the summary
/// request never has to carry the whole history.
pub(crate) async fn run_migration_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> MigrationOutcome {
    // Operator-tunable knobs (config: migration_compact_*), see FORK.md §5.3.
    let settings = turn_context.config.migration_compact;

    let mut replaced = false;
    let mut announced = false;
    // Whether this pass shrank the history at all; a pass whose summaries do
    // not reduce the surface must not be repeated (downshift plan §1).
    let mut progress = false;
    let pass_start_tokens = sess
        .clone_history()
        .await
        .estimate_token_count(turn_context.as_ref())
        .unwrap_or(0);

    let mut failure: Option<&'static str> = None;

    for _ in 0..settings.max_ranges {
        if cancellation.is_cancelled() {
            let outcome = MigrationOutcome::Cancelled;
            finish_announcement(sess.as_ref(), &turn_context, announced, outcome).await;
            return outcome;
        }
        let history = sess.clone_history().await;
        let status =
            crate::session::context_window::context_window_token_status(&sess, &turn_context).await;
        let Some(window) = status.full_context_window_limit else {
            break;
        };
        if window <= 0 {
            break;
        }
        let history_tokens = history
            .estimate_token_count(turn_context.as_ref())
            .unwrap_or(0);
        // Reported usage covers the fixed request prefix (instructions + tools) that
        // the history estimate does not, so charge it against the window instead of
        // pretending the whole window is available for history.
        let prefix_tokens = status
            .active_context_tokens
            .saturating_sub(history_tokens)
            .max(0);
        // The next request also has to fit `input + output`: reserve the
        // generation cap so the migration does not stop just below a request the
        // gateway will still reject. The same value is enforced as the summary
        // request's actual output cap (see summarize_migration_region), so the
        // reservation and the wire request cannot drift apart.
        let budget = ((window as f64 * settings.ratio) as i64)
            .saturating_sub(prefix_tokens)
            .saturating_sub(RESERVED_OUTPUT_TOKENS);
        if budget <= 0 {
            // The fixed prefix alone overflows the usable window: no span can
            // make the request fit, and no summary can either. Degrade decides
            // whether even the necessary request content fits.
            tracing::warn!(
                window,
                prefix_tokens,
                "migration compaction: no room left under the history budget"
            );
            failure = Some("no room under the history budget");
            break;
        }
        if history_tokens <= budget {
            break;
        }

        let retain = (window as f64 * settings.retain_ratio) as i64;
        // The summary request also carries the fixed prefix and the provider's
        // tool payload, and the bridge prices all of that before sending. Keep
        // the span to half the remaining history budget so the request cannot
        // trip that budget gate.
        let max_span = (((window as f64 * settings.max_span_ratio) as i64).min(budget / 2)).max(1);
        let items = history.annotated_items();
        let Some(region) =
            crate::session::compaction_region::select_compaction_region(items, retain, max_span)
        else {
            failure = Some("no pairing-safe range with an identified boundary");
            break;
        };

        // The record hides history by the id of the first item the model must
        // still see. The selector only returns cuts that already land on an
        // id carrier (pairing-, span-, and tail-safe), so the boundary is
        // directly usable; missing ids here are a selector contract breach.
        let boundary_id = match items
            .get(region.end)
            .and_then(|item| item.id())
            .map(|id| id.as_str().to_string())
        {
            Some(id) => id,
            None => {
                failure = Some("no identified history item to resume at");
                break;
            }
        };

        if !announced {
            // TUI and ACP both surface this item, so the user sees the
            // compaction instead of a silent pause.
            let item = TurnItem::ContextCompaction(ContextCompactionItem::new());
            sess.emit_turn_item_started(&turn_context, &item).await;
            announced = true;
        }
        let summary = match summarize_migration_region(
            &sess,
            &turn_context,
            &items[region.start..region.end],
        )
        .await
        {
            Ok(summary) => summary,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "migration compaction: summary request failed; degradation can still continue"
                );
                failure = Some("summary request failed");
                break;
            }
        };
        if summary.trim().is_empty() {
            failure = Some("summary was empty");
            break;
        }

        let record = codex_history::SurfaceReplacementItem {
            compaction_id: uuid::Uuid::new_v4().to_string(),
            resume_at_item_id: boundary_id,
            summary: summary.clone(),
            provider: turn_context.config.model_provider_id.clone(),
            model: turn_context.model_info().slug.clone(),
            shadowed_tokens: region.tokens,
            retained_tail_tokens: (window as f64 * settings.retain_ratio) as i64,
            reason: codex_history::SurfaceReplacementReason::WindowShrink,
            supersedes: Vec::new(),
            pruned_tool_outputs: Vec::new(),
        };
        sess.persist_rollout_items(&[codex_history::RolloutItem::SurfaceReplacement(Box::new(
            record,
        ))])
        .await;

        // Apply the same cut in memory so the rest of this turn sees the smaller
        // surface without waiting for a reload.
        let mut replacement = Vec::with_capacity(items.len() - region.end + 1);
        replacement.extend(items[region.end..].iter().cloned());
        replacement.insert(
            0,
            codex_history::ResponseItemEnvelope {
                item: ResponseItem::Message {
                    id: None,
                    role: "user".to_owned(),
                    content: vec![ContentItem::InputText {
                        text: format!("{SUMMARY_PREFIX}\n{summary}"),
                    }],
                    phase: None,
                    status: None,
                    encrypted_content: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                metadata: None,
            },
        );
        sess.replace_history_for_migration(replacement).await;
        replaced = true;
        progress = true;
        tracing::info!(
            shadowed_tokens = region.tokens,
            history_tokens,
            budget,
            window,
            "migration compaction: replaced the oldest history with a summary"
        );
    }

    let outcome = if failure.is_some() {
        MigrationOutcome::CannotSummarize
    } else {
        // Remeasure after the pass: converged, or still over budget (the
        // caller repeats the pass up to the per-turn cap, then degrades).
        let history_tokens = sess
            .clone_history()
            .await
            .estimate_token_count(turn_context.as_ref())
            .unwrap_or(0);
        let status =
            crate::session::context_window::context_window_token_status(&sess, &turn_context).await;
        let fits = status.full_context_window_limit.is_some_and(|window| {
            let prefix = status
                .active_context_tokens
                .saturating_sub(history_tokens)
                .max(0);
            let budget = ((window as f64 * settings.ratio) as i64)
                .saturating_sub(prefix)
                .saturating_sub(RESERVED_OUTPUT_TOKENS);
            budget > 0 && history_tokens <= budget
        });
        if fits {
            if replaced {
                MigrationOutcome::Converged
            } else {
                // The history already fit when the pass started; no migration
                // happened (the previous-model compact path stays eligible).
                MigrationOutcome::NoMigrationNeeded
            }
        } else if replaced
            || history_over_budget_at_entry(pass_start_tokens, &sess, &turn_context).await
        {
            MigrationOutcome::StillOverBudget
        } else {
            MigrationOutcome::NoMigrationNeeded
        }
    };
    if let Some(reason) = failure {
        tracing::warn!(reason, "migration compaction pass cannot summarize");
    }
    finish_announcement(sess.as_ref(), &turn_context, announced, outcome).await;
    outcome
}

/// Whether the history exceeded the migration budget when the pass started,
/// so a pass that replaced nothing still reports `StillOverBudget` (the caller
/// degrades instead of silently proceeding on an over-window request).
async fn history_over_budget_at_entry(
    pass_start_tokens: i64,
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
) -> bool {
    let settings = turn_context.config.migration_compact;
    let status =
        crate::session::context_window::context_window_token_status(sess, turn_context).await;
    let Some(window) = status.full_context_window_limit else {
        return false;
    };
    let prefix = status
        .active_context_tokens
        .saturating_sub(pass_start_tokens)
        .max(0);
    let budget = ((window as f64 * settings.ratio) as i64)
        .saturating_sub(prefix)
        .saturating_sub(RESERVED_OUTPUT_TOKENS);
    budget > 0 && pass_start_tokens > budget
}

async fn finish_announcement(
    sess: &Session,
    turn_context: &Arc<TurnContext>,
    announced: bool,
    _outcome: MigrationOutcome,
) {
    if announced {
        let item = TurnItem::ContextCompaction(ContextCompactionItem::new());
        sess.emit_turn_item_completed(turn_context, item).await;
    }
}

/// Why history is being trimmed without a summary.
///
/// Two callers share one implementation so the persistence/replay contract
/// stays single-sourced (see 2026-11-09 incident in
/// `my-docs/compact-unrecoverable-selfheal-plan-2026-11-09.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DegradeReason {
    /// The conversation moved to a smaller-window model (downshift path).
    ModelDownshift,
    /// Summarizing compaction could not fit the live window, so the oldest
    /// history is omitted to let the turn continue instead of failing.
    CompactionOverflow,
}

/// What the degraded trim must keep even when the budget says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DegradeProtection {
    /// Nothing is protected: the caller reserved room for pending input.
    None,
    /// Keep the newest user message. Mid-turn degradation runs while the turn is
    /// already executing that instruction, so cutting past it would let the next
    /// sampling succeed on a history that no longer contains the task.
    CurrentTurnInput,
}

/// True when this envelope is a user message recorded by `turn_id`.
fn is_user_message_of_turn(envelope: &ResponseItemEnvelope, turn_id: &str) -> bool {
    match &envelope.item {
        ResponseItem::Message {
            role,
            internal_chat_message_metadata_passthrough,
            ..
        } if role == "user" => {
            internal_chat_message_metadata_passthrough
                .as_ref()
                .and_then(|metadata| metadata.turn_id.as_deref())
                == Some(turn_id)
        }
        _ => false,
    }
}

/// The result of the degraded downshift path (downshift plan 2026-11-08 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DegradeOutcome {
    /// Older history was trimmed (or already fit): inference may proceed on
    /// the target model.
    Trimmed {
        /// Tokens of model-visible history removed without a summary.
        shadowed_tokens: i64,
        /// Tokens of the retained tail.
        retained_tokens: i64,
    },
    /// Instructions + tools + the current input + the generation reserve
    /// already exceed the window: no request can succeed, and the caller
    /// must surface an actionable error while keeping this turn's input.
    NecessaryContentExceedsWindow { window: i64 },
}

/// Floor charged on top of instructions and the current input for the tool
/// payload, context reinjection, and serialization overhead, none of which
/// are measurable from history alone. Scales with the window (a fixed floor
/// under-prices large tool catalogs); the real gateway remains the authority,
/// and the bounded overflow retry only fires before any reply exists.
const DEGRADE_FIXED_MARGIN_MIN_TOKENS: i64 = 2_048;

fn degrade_fixed_margin_tokens(window: i64) -> i64 {
    DEGRADE_FIXED_MARGIN_MIN_TOKENS.max(window / 8)
}

/// Trim the oldest model-visible history so the turn's inference request fits
/// the TARGET model's window. Used only after bounded migration summaries
/// could not converge (pass cap, no legal cut, or failed summaries): the old
/// model/provider is never consulted, and unconverged history is never handed
/// to the legacy whole-history compact.
///
/// The current turn's input is not in history yet (it is recorded after this
/// point), so reserving for it keeps it intact. The cut keeps tool calls
/// paired, lands on an identified boundary when possible, and an empty
/// retained tail is allowed. A no-summary replacement record is persisted so
/// replay trims to the same boundary; the original rollout is untouched.
pub(crate) async fn degrade_history_to_target_window(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    pending_input_tokens: i64,
    cancellation: &tokio_util::sync::CancellationToken,
    reason: DegradeReason,
    protection: DegradeProtection,
    // Fork (2026-11-09): a gateway that rejected a request below the advertised
    // window must also constrain the retry's trim budget, otherwise the retry
    // resends the same oversized payload.
    observed_window: Option<i64>,
) -> CodexResult<DegradeOutcome> {
    if cancellation.is_cancelled() {
        return Err(CodexErr::TurnAborted);
    }
    let settings = turn_context.config.migration_compact;
    let status =
        crate::session::context_window::context_window_token_status(sess, turn_context).await;
    let Some(window) = status
        .full_context_window_limit
        .filter(|window| *window > 0)
    else {
        // No known window: nothing to trim against.
        return Ok(DegradeOutcome::Trimmed {
            shadowed_tokens: 0,
            retained_tokens: 0,
        });
    };
    let instructions_tokens = (sess.get_prompt_base_instructions().await.text.len() as i64) / 4;
    // Two budgets with different meanings (2026-11-09 review P2):
    // - `history_budget` is the trim target; after a rejection it also honours
    //   the observed ceiling so the retry is actually smaller than the rejected
    //   payload;
    // - `necessary_budget` keeps the ADVERTISED window and decides whether the
    //   protected input itself is too large. A single image-heavy rejection must
    //   not turn a later text-only turn into "input too large".
    let advertised_window = window;
    let window = observed_window
        .filter(|observed| *observed > 0)
        .map_or(window, |observed| window.min(observed));
    let budget_for = |window: i64| {
        window
            .saturating_sub(RESERVED_OUTPUT_TOKENS)
            .saturating_sub(instructions_tokens)
            .saturating_sub(degrade_fixed_margin_tokens(window))
            .saturating_sub(pending_input_tokens.max(0))
    };
    let history_budget = budget_for(window);
    let necessary_budget = budget_for(advertised_window);
    if necessary_budget < 0 {
        tracing::warn!(
            window = advertised_window,
            instructions_tokens,
            pending_input_tokens,
            "downshift degrade: the necessary request content exceeds the target window"
        );
        return Ok(DegradeOutcome::NecessaryContentExceedsWindow {
            window: advertised_window,
        });
    }

    let history = sess.clone_history().await;
    let items = history.annotated_items();
    if items.is_empty() {
        return Ok(DegradeOutcome::Trimmed {
            shadowed_tokens: 0,
            retained_tokens: 0,
        });
    }
    let item_tokens: Vec<i64> = items
        .iter()
        .map(|envelope| crate::context_manager::estimate_item_token_count(&envelope.item))
        .collect();
    let total: i64 = item_tokens.iter().sum();
    if total <= history_budget {
        return Ok(DegradeOutcome::Trimmed {
            shadowed_tokens: 0,
            retained_tokens: total,
        });
    }

    // Largest retained tail fitting the budget: walk the boundary forward
    // from the oldest item until the remaining tail fits, then keep moving
    // forward to the nearest pairing-safe, id-carrying boundary. An empty
    // tail (boundary past the last item) is the final fallback.
    let prefix = crate::session::compaction_region::cut_balances(items);
    let mut boundary = 0usize;
    let mut tail: i64 = 0;
    for index in (0..items.len()).rev() {
        tail += item_tokens[index];
        if tail > history_budget {
            // Keep from index+1 onward; index itself does not fit.
            boundary = index + 1;
            break;
        }
    }
    while boundary < items.len() && (items[boundary].id().is_none() || prefix[boundary] != 0) {
        boundary += 1;
    }
    // Fork (2026-11-09): never cut past the instruction the running turn is
    // already executing. A long tool output can otherwise push the boundary
    // after the user's message (or to an empty tail), so the retried request
    // would succeed without the task it was asked to perform.
    let protected_index = match protection {
        DegradeProtection::None => None,
        DegradeProtection::CurrentTurnInput => {
            // Protect from the first user message of THIS turn: a steer arrives
            // as a second user message in the same turn, so clamping to the last
            // one could drop the task the turn is actually executing.
            let turn_id = turn_context.sub_id.as_str();
            items
                .iter()
                .position(|envelope| is_user_message_of_turn(envelope, turn_id))
                .or_else(|| {
                    items.iter().rposition(|envelope| {
                        matches!(&envelope.item, ResponseItem::Message { role, .. } if role == "user")
                    })
                })
        }
    };
    let boundary = match protected_index {
        Some(protected) if boundary > protected => protected,
        _ => boundary,
    };
    let boundary_id = items
        .get(boundary)
        .and_then(|item| item.id())
        .map(|id| id.as_str().to_string())
        .unwrap_or_default();
    let shadowed_tokens: i64 = item_tokens[..boundary].iter().sum();
    let mut retained: Vec<ResponseItemEnvelope> = items[boundary..].to_vec();
    let mut retained_tokens: i64 = item_tokens[boundary..].iter().sum();
    debug_assert_eq!(shadowed_tokens + retained_tokens, total);
    let mut pruned_tool_outputs: Vec<codex_history::PrunedToolOutput> = Vec::new();
    if protection == DegradeProtection::CurrentTurnInput && retained_tokens > history_budget {
        // The retained tail is usually dominated by tool output the turn can
        // re-run. Shrink that deterministically before declaring the protected
        // input itself too large: a one-line question followed by a huge tool
        // result must not fail as "input too large". The call ids are persisted
        // with the replacement so a reload reproduces the same model-visible
        // surface instead of restoring the full payload.
        let keep_head_chars = 4_000;
        let keep_tail_chars = 1_000;
        let call_ids = crate::session::compaction_prune::oversized_tool_output_call_ids(
            &retained,
            keep_head_chars + keep_tail_chars,
        );
        let pruned = crate::session::compaction_prune::prune_oversized_tool_results(
            &mut retained,
            keep_head_chars,
            keep_tail_chars,
            keep_head_chars + keep_tail_chars,
        );
        if !pruned.is_empty() {
            retained_tokens = retained
                .iter()
                .map(|envelope| crate::context_manager::estimate_item_token_count(&envelope.item))
                .sum();
            pruned_tool_outputs = call_ids
                .into_iter()
                .map(|call_id| codex_history::PrunedToolOutput {
                    call_id,
                    keep_head_chars,
                    keep_tail_chars,
                })
                .collect();
            tracing::info!(
                pruned_results = pruned.pruned_results,
                chars_removed = pruned.chars_removed,
                retained_tokens,
                "mid-turn degrade: pruned tool output in the retained tail"
            );
        }
        // Many tool calls in one turn: fixed windows may still overflow. Keep
        // shrinking the LARGEST remaining text output (function OR custom tool)
        // until the tail fits, nothing shrinks anymore, or the round cap hits
        // (2026-11-09 review P2). Every application goes through the SAME
        // projection helper the replay uses, and each window is recorded as its
        // own sequence entry so a reload reproduces this byte-for-byte
        // (review P2: live and replay must share one projection).
        let mut extra_rounds = 0u8;
        while retained_tokens > necessary_budget && extra_rounds < MAX_PRUNE_ESCALATION_ROUNDS {
            extra_rounds += 1;
            let mut largest: Option<(usize, i64)> = None;
            for (index, envelope) in retained.iter().enumerate() {
                let is_text_output = matches!(
                    &envelope.item,
                    ResponseItem::FunctionCallOutput { output, .. }
                        | ResponseItem::CustomToolCallOutput { output, .. }
                    if matches!(output.body, codex_protocol::models::FunctionCallOutputBody::Text(_))
                );
                if !is_text_output {
                    continue;
                }
                let tokens = crate::context_manager::estimate_item_token_count(&envelope.item);
                if largest.is_none_or(|(_, best)| tokens > best) {
                    largest = Some((index, tokens));
                }
            }
            let Some((index, _tokens)) = largest else {
                break;
            };
            let call_id = match &retained[index].item {
                ResponseItem::FunctionCallOutput { call_id, .. } => {
                    call_id.clone().unwrap_or_default()
                }
                ResponseItem::CustomToolCallOutput { call_id, .. } => call_id.clone(),
                _ => String::new(),
            };
            if call_id.is_empty() {
                break;
            }
            let current_chars = match &retained[index].item {
                ResponseItem::FunctionCallOutput { output, .. }
                | ResponseItem::CustomToolCallOutput { output, .. } => match &output.body {
                    codex_protocol::models::FunctionCallOutputBody::Text(text) => {
                        text.chars().count()
                    }
                    _ => break,
                },
                _ => break,
            };
            let next_total = (current_chars / 2).max(200);
            let next_head = (next_total * 4) / 5;
            let next_tail = next_total - next_head;
            let applied = crate::session::compaction_prune::prune_tool_output_by_call_id(
                &mut retained,
                &call_id,
                next_head,
                next_tail,
            );
            if !applied {
                break;
            }
            let new_tokens: i64 = retained
                .iter()
                .map(|envelope| crate::context_manager::estimate_item_token_count(&envelope.item))
                .sum();
            if new_tokens >= retained_tokens {
                // No further progress is possible (the output is at its stub).
                retained_tokens = new_tokens;
                break;
            }
            retained_tokens = new_tokens;
            pruned_tool_outputs.push(codex_history::PrunedToolOutput {
                call_id,
                keep_head_chars: next_head,
                keep_tail_chars: next_tail,
            });
        }
    }
    if protection == DegradeProtection::CurrentTurnInput && retained_tokens > necessary_budget {
        // The protected input plus everything after it cannot fit even in the
        // advertised window: no trim can make this request succeed.
        tracing::warn!(
            window = advertised_window,
            necessary_budget,
            retained_tokens,
            "mid-turn degrade: the current input alone exceeds the window"
        );
        return Ok(DegradeOutcome::NecessaryContentExceedsWindow {
            window: advertised_window,
        });
    }

    let record = codex_history::SurfaceReplacementItem {
        compaction_id: uuid::Uuid::new_v4().to_string(),
        // An empty boundary id means the retained tail is empty; the replay
        // side interprets it as "hide everything".
        resume_at_item_id: boundary_id,
        summary: String::new(),
        provider: turn_context.config.model_provider_id.clone(),
        model: turn_context.model_info().slug.clone(),
        shadowed_tokens,
        retained_tail_tokens: retained_tokens,
        // Downshift records the window-shrink flavor so replay tooling can
        // tell it apart from a live overflow recovery.
        reason: match reason {
            DegradeReason::ModelDownshift => {
                codex_history::SurfaceReplacementReason::WindowShrinkOmitted
            }
            DegradeReason::CompactionOverflow => codex_history::SurfaceReplacementReason::Overflow,
        },
        supersedes: Vec::new(),
        pruned_tool_outputs,
    };
    sess.persist_rollout_items(&[codex_history::RolloutItem::SurfaceReplacement(Box::new(
        record,
    ))])
    .await;

    // Apply the same cut in memory: the tail only, nothing planted.
    sess.replace_history_for_migration(retained).await;

    // One user-visible notice for the actual omission (spec: 提示一次).
    let notice = match reason {
        DegradeReason::ModelDownshift => format!(
            "The context window shrank after the model switch; ~{shadowed_tokens} tokens of older history were omitted (without a summary) so this turn can continue. The original conversation record is preserved."
        ),
        // Fork: the user did not switch anything — conversation compaction
        // itself could not fit, so name the harness and say what was lost.
        DegradeReason::CompactionOverflow => format!(
            "nuwax-harness could not summarize this conversation into the model's context window; ~{shadowed_tokens} tokens of older history were omitted (without a summary) so this turn can continue. The local record is complete; switch to a larger-window model if the model still needs that detail."
        ),
    };
    sess.send_event(
        turn_context,
        EventMsg::Warning(WarningEvent { message: notice }),
    )
    .await;
    tracing::info!(
        window,
        history_budget,
        shadowed_tokens,
        retained_tokens,
        settings_ratio = settings.ratio,
        ?reason,
        "degrade: trimmed older history without summaries"
    );
    Ok(DegradeOutcome::Trimmed {
        shadowed_tokens,
        retained_tokens,
    })
}

/// One summarization call over a bounded slice of history.
///
/// Builds the request the same way the full compaction task does
/// (`for_prompt_annotated` + provenance + executed-tool attachment): a bare
/// `Prompt` built by hand produced a stream with no final item on the chat wire.
async fn summarize_migration_region(
    sess: &Arc<Session>,
    turn_context: &TurnContext,
    region: &[codex_history::ResponseItemEnvelope],
) -> CodexResult<String> {
    let model_info = turn_context.model_info();
    let mut scratch = crate::context_manager::ContextManager::new();
    let items: Vec<ResponseItem> = region.iter().cloned().map(|item| item.item).collect();
    scratch.record_items(&items, model_info.truncation_policy.into());
    scratch.record_items(
        &[ResponseItem::Message {
            id: None,
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: SUMMARIZATION_PROMPT.to_string(),
            }],
            phase: None,
            status: None,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        }],
        model_info.truncation_policy.into(),
    );

    let annotated_input = scratch
        .for_prompt_annotated(
            &model_info.input_modalities,
            sess.services.image_store.as_ref(),
        )
        .await;
    let input_provenance = crate::model_output_projection::sources_for_input(&annotated_input);
    let mut turn_input = annotated_input
        .into_iter()
        .map(codex_history::ResponseItemEnvelope::into_item)
        .collect::<Vec<_>>();
    sess.services
        .executed_tool_calls
        .attach_to_compaction_prompt(&mut turn_input);

    let base_instructions = sess.get_prompt_base_instructions().await;
    let prompt = Prompt {
        input: turn_input,
        input_provenance,
        base_instructions,
        cyber_access_program: turn_context.cyber_access_program,
        // The span budget in `run_migration_compact_task` reserves exactly
        // RESERVED_OUTPUT_TOKENS for the summary output; enforce the same
        // value on the wire instead of inheriting the provider's (much
        // larger) default cap, or input+output can overflow the target
        // window the span was sized for.
        max_output_tokens: Some(
            u64::try_from(RESERVED_OUTPUT_TOKENS).expect("reserved output tokens as u64"),
        ),
        ..Default::default()
    };
    // `PostTurn` is what makes `drain_to_completed` hand the output back
    // instead of recording it into the conversation: migration compaction
    // consumes the summary itself and writes its own replacement record, so the
    // summary must not land in the thread's history.
    let metadata = CompactionTurnMetadata::new(
        CompactionTrigger::Auto,
        CompactionReason::ModelDownshift,
        CompactionImplementation::Responses,
        CompactionPhase::PostTurn,
    );
    let responses_metadata = sess
        .compaction_responses_metadata(turn_context, metadata)
        .await;
    let mut client_session = sess.services.model_client.new_session();
    let response_key = sess.next_sampling_response_key(&turn_context.sub_id);
    let response = drain_to_completed(
        sess,
        turn_context,
        &mut client_session,
        &responses_metadata,
        &prompt,
        CompactionPhase::PostTurn,
        &response_key,
    )
    .await?;

    if let Some(summary) = get_last_assistant_message_from_turn(response.output.iter()) {
        return Ok(summary);
    }
    // Some chat-wire models answer the summarization prompt as a plain message
    // once the response is replayable; take any text the turn produced.
    let fallback: String = response
        .output
        .iter()
        .rev()
        .filter_map(|item| match item {
            ResponseItem::Message { content, .. } => Some(content),
            _ => None,
        })
        .flat_map(|content| content.iter())
        .filter_map(|item| match item {
            ContentItem::OutputText { text, .. } | ContentItem::InputText { text } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_default();
    if fallback.is_empty() {
        let kinds: Vec<&str> = response
            .output
            .iter()
            .map(|item| match item {
                ResponseItem::Message { .. } => "message",
                ResponseItem::Reasoning { .. } => "reasoning",
                ResponseItem::FunctionCall { .. } => "function_call",
                _ => "other",
            })
            .collect();
        tracing::warn!(
            items = kinds.len(),
            kinds = ?kinds,
            "migration compaction: the summary response carried no text"
        );
    }
    Ok(fallback)
}
