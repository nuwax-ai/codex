//! Tool declaration and history names must use the same flattening rules.

use std::collections::HashMap;
use std::collections::HashSet;

use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolName;
use codex_tools::code_mode_name_for_tool_name;
use rig_core::completion::request::ToolDefinition;
use serde_json::Value;
use serde_json::json;

pub(crate) struct RequestTools {
    pub(crate) definitions: Vec<ToolDefinition>,
    pub(crate) meta: ToolMeta,
}

/// Request-scoped tool metadata the stream layer needs after the
/// definitions have been moved into the `CompletionRequest`: names of custom
/// tools (for `CustomToolCall` restoration) and per-tool `strict` flags (for
/// wire injection). Returned alongside the definitions so callers never have
/// to re-parse the tool list.
pub(crate) struct ToolMeta {
    pub(crate) custom_names: HashSet<String>,
    pub(crate) strict: HashMap<String, bool>,
    pub(crate) result_errors: HashMap<String, bool>,
    /// Raw hosted (server-side) tool declarations from the Responses request.
    /// Protocol-specific translation happens in `hosted_tools` (Anthropic
    /// server tools; Chat has no hosted-tool concept and drops them).
    pub(crate) hosted_tools: Vec<Value>,
    /// Persisted web-search wire pairs to replay into the serialized body:
    /// (index of the assistant rig message, raw blocks in order). Populated
    /// by history conversion on the Anthropic wire.
    pub(crate) websearch_replay: Vec<crate::hosted_replay::ReplayGroup>,
}

pub(crate) fn flat_name(name: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(namespace) => code_mode_name_for_tool_name(&ToolName::namespaced(namespace, name)),
        None => name.to_string(),
    }
}

pub(crate) fn request_tools(
    request: &ResponsesApiRequest,
) -> Result<RequestTools, codex_api::ApiError> {
    let mut tools: Vec<Value> = match request.tools.as_ref() {
        None => Vec::new(),
        Some(tools) => {
            // Core produces RawValue directly from typed schemas. Valid JSON
            // can exceed Value's depth or exact integer range; never replace
            // the entire tool list with empty tools or rounded constraints.
            let raw = serde_json::to_string(tools).map_err(tool_definition_error)?;
            if has_out_of_range_integer(&raw) {
                return Err(codex_api::ApiError::InvalidRequest {
                    message: "Tool definitions contain an integer outside the exact range supported by the Rig bridge".into(),
                });
            }
            serde_json::from_str(&raw).map_err(tool_definition_error)?
        }
    };
    for item in &request.input {
        if let ResponseItem::AdditionalTools { tools: extra, .. } = item {
            tools.extend(extra.iter().cloned());
        }
    }
    let mut selection = parse_tools(&tools)?;
    for item in &request.input {
        let (call_id, output) = match item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                output,
                ..
            }
            | ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => (call_id, output),
            _ => continue,
        };
        if let Some(success) = output.success {
            selection
                .meta
                .result_errors
                .insert(call_id.clone(), !success);
        }
    }
    Ok(selection)
}

fn tool_definition_error(error: serde_json::Error) -> codex_api::ApiError {
    // Displaying a serde data error may include a rejected string value.
    // Keep the diagnostic category and position without copying tool content.
    codex_api::ApiError::InvalidRequest {
        message: format!(
            "Cannot decode tool definitions for the Rig bridge ({:?} error at line {} column {})",
            error.classify(),
            error.line(),
            error.column()
        ),
    }
}

/// Inspect valid raw JSON before Value can round an integer into an f64.
/// Decimal/exponent numbers already select floating-point semantics; only
/// integer tokens outside both i64 and u64 need rejection. Quoted strings,
/// including escaped quotes and backslashes, are never interpreted as numbers.
fn has_out_of_range_integer(json: &str) -> bool {
    let bytes = json.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    index += 1;
                }
                let token = &json[start..index];
                if !token.contains(['.', 'e', 'E'])
                    && token.parse::<i64>().is_err()
                    && token.parse::<u64>().is_err()
                {
                    return true;
                }
            }
            _ => index += 1,
        }
    }
    false
}

pub(crate) fn parse_tools(tools: &[Value]) -> Result<RequestTools, codex_api::ApiError> {
    let mut definitions = Vec::new();
    let mut custom = HashSet::new();
    let mut strict = HashMap::new();
    let mut hosted_tools = Vec::new();
    for tool in tools {
        if tool["type"] == "namespace" {
            if let (Some(namespace), Some(children)) =
                (tool["name"].as_str(), tool["tools"].as_array())
            {
                for child in children {
                    append_tool(
                        child,
                        Some(namespace),
                        &mut definitions,
                        &mut custom,
                        &mut strict,
                        &mut hosted_tools,
                    )?;
                }
            }
        } else {
            append_tool(
                tool,
                None,
                &mut definitions,
                &mut custom,
                &mut strict,
                &mut hosted_tools,
            )?;
        }
    }
    Ok(RequestTools {
        definitions,
        meta: ToolMeta {
            custom_names: custom,
            strict,
            result_errors: HashMap::new(),
            hosted_tools,
            websearch_replay: Vec::new(),
        },
    })
}

fn append_tool(
    tool: &Value,
    namespace: Option<&str>,
    definitions: &mut Vec<ToolDefinition>,
    custom: &mut HashSet<String>,
    strict: &mut HashMap<String, bool>,
    hosted_tools: &mut Vec<Value>,
) -> Result<(), codex_api::ApiError> {
    // Hosted (server-side) tools carry no function schema and no `name`;
    // they never become function definitions. Keep the raw declaration for
    // per-protocol translation by `hosted_tools`.
    if !matches!(tool["type"].as_str(), Some("function") | Some("custom")) {
        hosted_tools.push(tool.clone());
        return Ok(());
    }
    let Some(name) = tool["name"].as_str() else {
        return Ok(());
    };
    let name = flat_name(name, namespace);
    let parameters = match tool["type"].as_str() {
        Some("custom") => {
            custom.insert(name.clone());
            json!({"type":"object", "properties":{"input":{"type":"string", "description":"The complete tool input text"}}, "required":["input"], "additionalProperties":false})
        }
        Some("function") => tool
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object", "properties":{}})),
        // Unreachable: hosted tool types returned above.
        _ => return Ok(()),
    };
    if let Some(value) = tool["strict"].as_bool() {
        strict.insert(name.clone(), value);
    }
    // Custom tools carry their contract in the wrapper schema; a missing
    // description still needs to tell the model how to use the input field.
    let fallback_description = if tool["type"].as_str() == Some("custom") {
        "Custom tool; pass the complete tool input text in the `input` field."
    } else {
        ""
    };
    let tool_name = rig_core::completion::message::ToolName::new(name.clone()).map_err(|_| {
        codex_api::ApiError::InvalidRequest {
            message: format!("Tool name {name:?} must not be empty"),
        }
    })?;
    definitions.push(ToolDefinition {
        name: tool_name,
        description: tool["description"]
            .as_str()
            .filter(|description| !description.is_empty())
            .unwrap_or(fallback_description)
            .to_string(),
        parameters,
    });
    Ok(())
}

/// A history tool call's flat wire name as a `ToolName`.
pub(crate) fn flat_tool_name(
    name: &str,
    namespace: Option<&str>,
) -> Result<rig_core::completion::message::ToolName, codex_api::ApiError> {
    rig_core::completion::message::ToolName::new(flat_name(name, namespace)).map_err(|_| {
        codex_api::ApiError::InvalidRequest {
            message: format!("Historical tool {name:?} flattened name must not be empty"),
        }
    })
}
