use super::*;
use crate::agent::api::AgentInput;
use crate::agent::api::SpawnRequest;
use crate::agent::child_config::SpawnConfigOptions;
use crate::agent::child_config::SpawnConfigVersion;
use crate::agent::child_config::prepare_agent_spawn_config;
use crate::agent::control::render_input_preview;
use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::agent::role::DEFAULT_ROLE_NAME;
use crate::agent::types::SpawnAgentForkMode;
use crate::agent::types::SpawnAgentOptions;
use crate::tools::handlers::multi_agents_spec::SpawnAgentToolOptions;
use crate::tools::handlers::multi_agents_spec::create_spawn_agent_tool_v1;
use codex_protocol::protocol::MultiAgentVersion;
use codex_tools::ToolSpec;

#[derive(Default)]
pub(crate) struct Handler {
    options: SpawnAgentToolOptions,
}

impl Handler {
    pub(crate) fn new(options: SpawnAgentToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(MULTI_AGENT_V1_NAMESPACE, "spawn_agent")
    }

    fn spec(&self) -> ToolSpec {
        create_spawn_agent_tool_v1(self.options.clone())
    }

    fn search_info(&self) -> Option<ToolSearchInfo> {
        multi_agent_tool_search_info(
            "spawn_agent spawn agent subagent sub-agent delegate delegation parallel work worker explorer no-apps fork model reasoning",
            self.spec(),
        )
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move { handle_spawn_agent(invocation).await.map(boxed_tool_output) })
    }
}

async fn handle_spawn_agent(
    invocation: ToolInvocation,
) -> Result<SpawnAgentResult, FunctionCallError> {
    let ToolInvocation {
        session,
        step_context,
        payload,
        call_id,
        ..
    } = invocation;
    let turn = &step_context.turn;
    let arguments = function_arguments(payload)?;
    let args: SpawnAgentArgs = parse_arguments(&arguments)?;
    let role_name = args
        .agent_type
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());
    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = render_input_preview(&input_items);
    let session_source = turn.session_source.clone();
    let child_depth = next_thread_spawn_depth(&session_source);
    let max_depth = turn.config.agent_max_depth;
    if exceeds_thread_spawn_depth_limit(child_depth, max_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Agent depth limit reached. Solve the task yourself.".to_string(),
        ));
    }
    session
        .emit_turn_item_started(
            turn,
            &TurnItem::CollabAgentToolCall(CollabAgentToolCallItem {
                id: call_id.clone(),
                tool: CollabAgentTool::SpawnAgent,
                status: CollabAgentToolCallStatus::InProgress,
                sender_thread_id: session.thread_id,
                receiver_thread_ids: Vec::new(),
                receiver_agents: Vec::new(),
                prompt: Some(prompt.clone()),
                model: Some(args.model.clone().unwrap_or_default()),
                // Fork (2026-11-10): an omitted effort must stay omitted. The
                // `unwrap_or_default()` this replaces injected `Medium` (the
                // enum's `#[default]`) into the sub-agent's session, which the
                // tier gate then refused on gateways without a `medium` tier
                // (GLM-5.3 takes low/high/max) — the same class of bug as the
                // adapter's hardcoded fallback.
                reasoning_effort: args.reasoning_effort.clone(),
                agents_states: Default::default(),
            }),
        )
        .await;
    let prepared = prepare_agent_spawn_config(
        &session,
        step_context.as_ref(),
        SpawnConfigOptions {
            version: SpawnConfigVersion::V1,
            full_history_fork: args.fork_context,
            role_name,
            model: args.model.as_deref(),
            reasoning_effort: args.reasoning_effort.clone(),
        },
    )
    .await
    .map_err(FunctionCallError::RespondToModel)?;
    let config = prepared.config;
    let fork_mode = args.fork_context.then_some(SpawnAgentForkMode::FullHistory);
    let result = session
        .services
        .agent_control
        .spawn(SpawnRequest {
            caller: session.thread_id,
            config,
            input: AgentInput::UserInput(input_items),
            source: thread_spawn_source(
                session.thread_id,
                &turn.session_source,
                child_depth,
                prepared.role_name.as_deref(),
                /*task_name*/ None,
            )?,
            options: SpawnAgentOptions {
                fork_parent_spawn_call_id: fork_mode.as_ref().map(|_| call_id.clone()),
                fork_mode: fork_mode.clone(),
                parent_thread_id: Some(session.thread_id),
                parent_turn_id: Some(turn.sub_id.clone()),
                root_turn_id: turn.turn_metadata_state.root_turn_id(),
                turn_trigger: turn.turn_metadata_state.current_turn_trigger(),
                environments: Some(step_context.inherited_environments.clone()),
                disabled_plugin_ids: Some(turn.disabled_plugin_ids.clone()),
                multi_agent_v2_usage_hints: None,
                cyber_access_program: turn.cyber_access_program,
            },
        })
        .await
        .map_err(|err| {
            record_collab_spawn_failure(
                &turn.session_telemetry,
                turn.config.apps_mcp_product_sku.as_deref(),
                &err,
                &call_id,
                &turn.sub_id,
                fork_mode.as_ref(),
                MultiAgentVersion::V1,
            );
            collab_spawn_error(err)
        });
    let (new_thread_id, status) = match &result {
        Ok((spawned_agent, _)) => (Some(spawned_agent.thread_id), spawned_agent.status.clone()),
        Err(_) => (None, AgentStatus::NotFound),
    };
    let agent_snapshot = result.as_ref().ok().map(|(_, config)| config);
    let new_agent_nickname =
        agent_snapshot.and_then(|snapshot| snapshot.session_source.get_nickname());
    let new_agent_role =
        agent_snapshot.and_then(|snapshot| snapshot.session_source.get_agent_role());
    let effective_model = agent_snapshot
        .map(|snapshot| snapshot.model.clone())
        .unwrap_or_else(|| args.model.clone().unwrap_or_default());
    // Fork (2026-11-10): `Option` end to end — a sub-agent the caller did not
    // pin inherits the parent's model default behavior (the engine omits the
    // field on the wire) instead of an invented tier.
    let effective_reasoning_effort: Option<ReasoningEffort> = effective_spawn_effort(
        agent_snapshot.and_then(|snapshot| snapshot.reasoning_effort.clone()),
        args.reasoning_effort.clone(),
    );
    let nickname = new_agent_nickname.clone();
    let receiver_thread_ids = new_thread_id.into_iter().collect();
    let receiver_agents = new_thread_id
        .map(|thread_id| CollabAgentRef {
            thread_id,
            agent_nickname: new_agent_nickname,
            agent_role: new_agent_role,
        })
        .into_iter()
        .collect();
    let agents_states = new_thread_id
        .map(|thread_id| [(thread_id, status.clone())].into_iter().collect())
        .unwrap_or_default();
    session
        .emit_turn_item_completed(
            turn,
            TurnItem::CollabAgentToolCall(CollabAgentToolCallItem {
                id: call_id,
                tool: CollabAgentTool::SpawnAgent,
                status: collab_tool_call_status(&status, new_thread_id),
                sender_thread_id: session.thread_id,
                receiver_thread_ids,
                receiver_agents,
                prompt: Some(prompt),
                model: Some(effective_model),
                reasoning_effort: effective_reasoning_effort,
                agents_states,
            }),
        )
        .await;
    let (spawned_agent, _) = result?;
    let new_thread_id = spawned_agent.thread_id;
    let role_tag = role_name.unwrap_or(DEFAULT_ROLE_NAME);
    turn.session_telemetry.counter(
        "codex.multi_agent.spawn",
        /*inc*/ 1,
        &[("role", role_tag), ("version", "v1")],
    );

    Ok(SpawnAgentResult {
        agent_id: new_thread_id.to_string(),
        nickname,
    })
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

/// The effort a spawned sub-agent runs with: the caller's explicit pin, else
/// the parent snapshot's, else nothing.
///
/// Fork (2026-11-10): the last arm used to be `unwrap_or_default()`, which
/// injected `ReasoningEffort::Medium` (the enum's `#[default]`) whenever the
/// caller omitted the argument. The tier gate then refused that invented tier
/// on gateways without a `medium` level (GLM-5.3 takes low/high/max) — the same
/// class of bug as the ACP adapter's hardcoded fallback. `None` now stays
/// `None`, and the engine omits the field so the vendor applies its own
/// default.
fn effective_spawn_effort(
    snapshot_effort: Option<ReasoningEffort>,
    requested_effort: Option<ReasoningEffort>,
) -> Option<ReasoningEffort> {
    // Fork (2026-11-11): prefer the SNAPSHOT — it is the config the sub-agent
    // actually started with. The caller's argument is applied first and a role
    // profile can override it afterwards (`agent::role`), so "requested" is not
    // evidence of what ran: pinning `low` on a role that fixes `high` used to
    // report `low` for a thread running `high`. The argument only fills the gap
    // when the snapshot carries no explicit tier.
    snapshot_effort.or(requested_effort)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_omitted_effort_stays_omitted() {
        assert_eq!(effective_spawn_effort(None, None), None);
    }

    #[test]
    fn the_started_config_wins_over_the_callers_pin() {
        // A role profile can override the caller's pin, so the snapshot is the
        // only evidence of what the sub-agent actually ran with.
        assert_eq!(
            effective_spawn_effort(Some(ReasoningEffort::High), Some(ReasoningEffort::Low)),
            Some(ReasoningEffort::High),
        );
    }

    #[test]
    fn the_parent_snapshot_applies_when_the_caller_omits_it() {
        assert_eq!(
            effective_spawn_effort(Some(ReasoningEffort::High), None),
            Some(ReasoningEffort::High),
        );
    }
}

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    agent_type: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    fork_context: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct SpawnAgentResult {
    agent_id: String,
    nickname: Option<String>,
}

impl ToolOutput for SpawnAgentResult {
    fn log_output(&self) -> String {
        tool_output_json_text(self, "spawn_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "spawn_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "spawn_agent")
    }
}
