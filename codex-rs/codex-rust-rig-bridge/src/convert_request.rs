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

pub(crate) fn responses_request_to_completion_request(
    request: &ResponsesApiRequest,
    protocol: RigProtocol,
    source: &str,
) -> Result<(CompletionRequest, crate::request_tools::ToolMeta), ApiError> {
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
            format
                .map(|format| serde_json::from_value(format.schema.clone()))
                .transpose()
                .map_err(|error| ApiError::InvalidRequest {
                    message: format!("Invalid output schema: {error}"),
                })?
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

#[cfg(test)]
#[path = "convert_request_tests.rs"]
mod tests;
