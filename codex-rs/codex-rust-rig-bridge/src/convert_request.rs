//! Protocol-specific request controls. History and tool conversion live in sibling modules.
use crate::client::RigProtocol;
use crate::request_messages::convert_response_items;
use codex_api::ApiError;
use codex_api::ResponsesApiRequest;
use rig_core::completion::CompletionRequest;
use rig_core::completion::message::Message;
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
    let mut chat_history = convert_response_items(&request.input, protocol, source)?;
    if chat_history.is_empty() {
        return Err(ApiError::InvalidRequest {
            message: "No convertible messages in request".into(),
        });
    }
    if !request.instructions.is_empty() {
        chat_history.insert(
            0,
            Message::System {
                content: request.instructions.clone(),
            },
        );
    }
    let selection = crate::request_tools::request_tools(request);
    let mut params = serde_json::Map::new();
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
    Ok((
        CompletionRequest {
            model: Some(request.model.clone()),
            preamble: None,
            chat_history,
            documents: Vec::new(),
            tools: selection.definitions,
            temperature: None,
            max_tokens: (protocol == RigProtocol::Anthropic)
                .then_some(crate::client::DEFAULT_ANTHROPIC_MAX_TOKENS),
            tool_choice: map_tool_choice(&request.tool_choice),
            additional_params: (!params.is_empty()).then_some(Value::Object(params)),
            output_schema,
            record_telemetry_content: false,
        },
        selection.meta,
    ))
}

fn map_tool_choice(choice: &str) -> Option<ToolChoice> {
    Some(match choice {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" => ToolChoice::Required,
        other => ToolChoice::Specific {
            function_names: vec![other.to_string()],
        },
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
