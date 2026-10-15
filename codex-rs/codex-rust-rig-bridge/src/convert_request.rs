//! Protocol-specific request controls. History and tool conversion live in sibling modules.
use crate::client::RigProtocol;
use crate::request_messages::convert_response_items;
use codex_api::ApiError;
use codex_api::ResponsesApiRequest;
use rig_core::completion::CompletionRequest;
use rig_core::completion::message::ToolChoice;
use serde_json::Value;
use serde_json::json;

/// Shared by the entry guard and the exhaustive protocol match below: the
/// Responses wire is served by the same-protocol passthrough, never here.
const RESPONSES_NOT_CONVERTIBLE_MSG: &str = "the Chat/Anthropic conversion pipeline \
                                             cannot serve the Responses wire; use the \
                                             same-protocol passthrough in `responses.rs`";

pub(crate) fn responses_request_to_completion_request(
    request: &ResponsesApiRequest,
    protocol: RigProtocol,
    source: &str,
) -> Result<(CompletionRequest, crate::request_tools::ToolMeta), ApiError> {
    if protocol == RigProtocol::Responses {
        return Err(ApiError::InvalidRequest {
            message: RESPONSES_NOT_CONVERTIBLE_MSG.into(),
        });
    }
    if !request.stream {
        return Err(ApiError::InvalidRequest {
            message: "The Rig bridge requires a streaming request".into(),
        });
    }
    // Log names only: metadata, cache keys and payloads can be sensitive.
    let mut omitted = Vec::new();
    if request
        .reasoning
        .as_ref()
        .is_some_and(|value| value.summary.is_some())
    {
        omitted.push("reasoning.summary");
    }
    if request
        .reasoning
        .as_ref()
        .is_some_and(|value| value.context.is_some())
    {
        omitted.push("reasoning.context");
    }
    if !request.include.is_empty() {
        omitted.push("include");
    }
    if request.stream_options.is_some() {
        omitted.push("stream_options (Rig supplies its own)");
    }
    if request.client_metadata.is_some() {
        omitted.push("client_metadata");
    }
    if request.access_programs.is_some() {
        omitted.push("access_programs");
    }
    if protocol == RigProtocol::Anthropic {
        if request.store {
            omitted.push("store");
        }
        if request.prompt_cache_key.is_some() {
            omitted.push("prompt_cache_key");
        }
        if let Some(text) = &request.text {
            if text.verbosity.is_some() {
                omitted.push("text.verbosity");
            }
            if text.format.is_some() {
                omitted.push("text.format.name/strict (Anthropic format has neither)");
            }
        }
    }
    if !omitted.is_empty() {
        tracing::debug!(
            ?protocol,
            ?omitted,
            "Responses controls without a Rig wire equivalent"
        );
    }
    let mut websearch_replay = Vec::new();
    let mut chat_history =
        convert_response_items(&request.input, protocol, source, &mut websearch_replay)?;
    if chat_history.is_empty() {
        return Err(ApiError::InvalidRequest {
            message: "No convertible messages in request".into(),
        });
    }
    let selection = crate::request_tools::request_tools(request)?;
    let advertised: Vec<&str> = selection
        .definitions
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    validate_tool_choice(&request.tool_choice, &advertised)?;
    let mut params = serde_json::Map::new();
    // Hosted server tools ride as raw `additional_params["tools"]`: rig 0.44's
    // encoder serializes them verbatim into the Anthropic tools array, and
    // their presence marks the request as declaring tools so `adapt` keeps
    // tool-call history as calls instead of folding it into text.
    if protocol == RigProtocol::Anthropic {
        let server_tools =
            crate::hosted_tools::translate_anthropic_server_tools(&selection.meta.hosted_tools)?;
        if !server_tools.is_empty() {
            params.insert("tools".into(), json!(server_tools));
        }
    }
    let format = request.text.as_ref().and_then(|text| text.format.as_ref());
    let output_schema = match protocol {
        RigProtocol::Chat => {
            if let Some(format) = format {
                // The typed Rig schema both gates first-tool-turn output and forces
                // strict=true/name rewriting. Preserve the caller's full wire contract.
                params.insert("response_format".into(), json!({
                    "type": "json_schema",
                    "json_schema": { "name": format.name, "strict": format.strict, "schema": format.schema }
                }));
            }
            if let Some(verbosity) = request
                .text
                .as_ref()
                .and_then(|text| text.verbosity.as_ref())
            {
                params.insert("verbosity".into(), json!(verbosity));
            }
            if let Some(effort) = request
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort.as_ref())
            {
                params.insert("reasoning_effort".into(), json!(effort.as_str()));
            }
            if let Some(tier) = &request.service_tier {
                params.insert("service_tier".into(), json!(tier));
            }
            if let Some(key) = &request.prompt_cache_key {
                params.insert("prompt_cache_key".into(), json!(key));
            }
            params.insert(
                "parallel_tool_calls".into(),
                json!(request.parallel_tool_calls),
            );
            params.insert("store".into(), json!(request.store));
            None
        }
        RigProtocol::Anthropic => {
            // OpenAI knobs must not leak onto the Messages wire. Anthropic has
            // its own tool_choice parallelism control, applied below the SDK.
            if let Some(format) = format {
                // Rig's typed schema sanitizer makes optional properties required
                // and removes numeric constraints. Keep the caller's schema:
                // unsupported constraints should fail at the provider, not vanish.
                params.insert(
                    "output_config".into(),
                    json!({
                        "format": { "type": "json_schema", "schema": format.schema }
                    }),
                );
            }
            None
        }
        // Exhaustiveness: rejected by the entry guard above.
        RigProtocol::Responses => {
            return Err(ApiError::InvalidRequest {
                message: RESPONSES_NOT_CONVERTIBLE_MSG.into(),
            });
        }
    };
    // Authoritative request-side caps and shape checks (spec §3): count
    // real pairs — not assistant groups — reject malformed payloads, and
    // drop oversized or oldest pairs whole. Runs on every request build, so
    // resumed or imported history passes the same validation.
    let websearch_replay = crate::hosted_replay::sanitize_for_request(websearch_replay);
    let mut selection = selection;
    selection.meta.websearch_replay = websearch_replay;
    // `CompletionRequest` is `#[non_exhaustive]`; start from the public
    // constructor and layer the converted fields on top so new upstream
    // fields keep their defaults.
    let prompt = chat_history.pop().ok_or_else(|| ApiError::InvalidRequest {
        message: "No convertible messages in request".into(),
    })?;
    let mut completion = CompletionRequest::new(prompt);
    chat_history.extend(std::mem::take(&mut completion.chat_history));
    completion.chat_history = chat_history;
    completion.model = Some(request.model.clone());
    completion.tools = selection.definitions;
    completion.max_tokens =
        (protocol == RigProtocol::Anthropic).then_some(crate::client::DEFAULT_ANTHROPIC_MAX_TOKENS);
    completion.tool_choice = map_tool_choice(&request.tool_choice)?;
    completion.additional_params = (!params.is_empty()).then_some(Value::Object(params));
    completion.output_schema = output_schema;
    completion.record_telemetry_content = false;
    // rig 0.44's history adapter replays a turn's provider items only when
    // the turn's origin matches the request's wire, provider and model (and
    // context, on models that do not bind items to it). History the bridge
    // rebuilt is by construction from this same endpoint, so every assistant
    // turn claims that origin; the per-source envelopes already gate what is
    // replayed.
    if protocol == RigProtocol::Anthropic {
        let origin = anthropic_history_origin(&completion, &request.model);
        for message in &mut completion.chat_history {
            if let rig_core::completion::message::Message::Assistant(turn) = message {
                turn.origin = Some(origin.clone());
            }
        }
    }
    Ok((completion, selection.meta))
}

/// The `Origin` of a rebuilt history assistant turn on the Anthropic wire.
/// Mirrors the driver's own `Same` inputs: wire, provider descriptor, the
/// request's model, and — unless the model binds its items to the request
/// context — the context fingerprint below. Keep in sync with the pinned
/// rig version's `Operation::prepare`/`history::adapt_for`.
fn anthropic_history_origin(
    completion: &CompletionRequest,
    model: &str,
) -> rig_core::completion::message::Origin {
    use rig_core::completion::message::Api;
    use rig_core::completion::message::Origin;
    // A model that binds its items to the request context asks the provider
    // to drop unbound ones, so the driver compares no fingerprint there; a
    // request with no `thinking` option is adaptive by default.
    let binds_context = rig_core::providers::anthropic::completion::claude_spec(model)
        .is_some_and(|spec| spec.compat.binds_context);
    Origin {
        // The Messages wire's API name (`wire::MESSAGES_API`).
        api: Api::from("anthropic.messages"),
        provider: rig_core::providers::anthropic::PROVIDER_NAME.to_string(),
        model: model.to_string(),
        response_model: None,
        response_id: None,
        context: (!binds_context).then(|| anthropic_context_fingerprint(completion, model)),
    }
}

/// Mirrors rig 0.44's private `history::context_of` for the Messages wire:
/// the fingerprint of the sorted tool definitions, the leading system
/// prompt as the wire sends it, and the raw hosted tools. A fingerprint
/// mismatch only downgrades the turn to plain text, so a drift here fails
/// visibly in the reasoning-replay wire tests rather than corrupting a
/// request.
fn anthropic_context_fingerprint(
    completion: &CompletionRequest,
    model: &str,
) -> rig_core::completion::message::Fingerprint {
    use rig_core::completion::message::Fingerprint;
    use rig_core::completion::message::Message;
    let claude_spec = rig_core::providers::anthropic::completion::claude_spec(model);
    // The wire folds every system message into one unless the model takes
    // them mid-conversation.
    let folds = !claude_spec.is_some_and(|spec| spec.compat.mid_conversation_system);
    let mut tools: Vec<_> = completion.tools.iter().collect();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    let mut system: Vec<&str> = completion
        .chat_history
        .iter()
        .map_while(|message| match message {
            Message::System { content } => Some(Some(content.as_str())),
            _ => folds.then_some(None),
        })
        .flatten()
        .filter(|content| !content.trim().is_empty())
        .collect();
    let joined;
    if folds && system.len() > 1 {
        joined = system.join("\n\n");
        system = vec![joined.as_str()];
    }
    let raw_tools: &[Value] = completion
        .additional_params
        .as_ref()
        .and_then(|params| params.get("tools"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut fields = vec![json!("context"), json!(tools)];
    fields.push(json!(system));
    if !raw_tools.is_empty() {
        fields.push(json!(raw_tools));
    }
    Fingerprint::of(&Value::Array(fields))
}

fn map_tool_choice(choice: &str) -> Result<Option<ToolChoice>, ApiError> {
    Ok(Some(match choice {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" => ToolChoice::Required,
        other => ToolChoice::Specific {
            function_names: vec![
                rig_core::completion::message::ToolName::new(other.to_string()).map_err(|_| {
                    ApiError::InvalidRequest {
                        message: "tool_choice references a tool with an empty name".into(),
                    }
                })?,
            ],
        },
    }))
}

/// Fails fast when the Responses `tool_choice` cannot be honored on the
/// chat-family wire: `required` or a specific tool name needs at least one
/// advertised function tool. Hosted (server-side) tools cannot be forced, so
/// a hosted-only request cannot satisfy those choices — an explicit error
/// beats silently widening to `auto`.
pub(crate) fn validate_tool_choice(
    choice: &str,
    function_tool_names: &[&str],
) -> Result<(), ApiError> {
    match choice {
        "auto" | "none" => Ok(()),
        "required" if function_tool_names.is_empty() => Err(ApiError::InvalidRequest {
            message: "tool_choice \"required\" needs at least one function tool; hosted \
                      server tools cannot be forced — use \"auto\" or \"none\""
                .into(),
        }),
        "required" => Ok(()),
        name if !function_tool_names.contains(&name) => Err(ApiError::InvalidRequest {
            message: format!(
                "tool_choice references tool {name:?} which is not an advertised function tool"
            ),
        }),
        _ => Ok(()),
    }
}

/// The Anthropic wire shape of the Responses `tool_choice`, restored by the
/// transport when post-injected server tools left the serialized body without
/// one (rig's streaming path drops `tool_choice` when its typed tool list is
/// empty, and the transport re-advertises the tools afterwards).
pub(crate) fn anthropic_tool_choice(
    choice: &str,
    function_tool_names: &[&str],
) -> Result<Value, ApiError> {
    validate_tool_choice(choice, function_tool_names)?;
    Ok(match choice {
        "auto" => json!({"type": "auto"}),
        "none" => json!({"type": "none"}),
        "required" => json!({"type": "any"}),
        name => json!({"type": "tool", "name": name}),
    })
}

/// Maps a codex reasoning effort onto Anthropic's `output_config.effort`
/// scale (low/medium/high/xhigh/max). Values with no Anthropic counterpart
/// (`persistent`, unknown customs) stay unset. Explicit `none` disables
/// thinking separately; effort itself does not enable thinking.
pub(crate) fn anthropic_effort(request: &ResponsesApiRequest) -> Option<String> {
    use codex_protocol::openai_models::ReasoningEffort;
    let effort = request.reasoning.as_ref()?.effort.as_ref()?;
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => Some("low".into()),
        ReasoningEffort::Medium => Some("medium".into()),
        ReasoningEffort::High => Some("high".into()),
        ReasoningEffort::XHigh => Some("xhigh".into()),
        ReasoningEffort::Max => Some("max".into()),
        ReasoningEffort::Ultra => {
            tracing::warn!("reasoning effort `ultra` has no Anthropic level; clamping to `max`");
            Some("max".into())
        }
        ReasoningEffort::None => None,
        ReasoningEffort::Persistent | ReasoningEffort::Custom(_) => {
            tracing::warn!(
                "Requested reasoning effort has no Anthropic equivalent; omitting effort"
            );
            None
        }
    }
}

/// Only the semantically matching OpenAI tiers cross over; `flex` and
/// `priority` are OpenAI pricing concepts with no Messages equivalent.
pub(crate) fn anthropic_service_tier(request: &ResponsesApiRequest) -> Option<String> {
    match request.service_tier.as_deref() {
        Some("auto") => Some("auto".into()),
        Some("default" | "standard") => Some("standard_only".into()),
        Some(_) => {
            tracing::warn!("Requested service tier has no Anthropic equivalent; omitting it");
            None
        }
        None => None,
    }
}

#[cfg(test)]
#[path = "convert_request_tests.rs"]
mod tests;
